//! The small sorter: a 12-layer multilingual BERT encoder (from
//! multilingual-e5-small, MIT) fine-tuned for Email as a cross-encoder. It
//! reads a filter description and an email together and returns the
//! probability that the email belongs in that filter. There is no text
//! generation, so message text can only move the score.
//!
//! The file format is one safetensors file: word embeddings as int8 per row
//! (u8 with a +128 offset and an f16 scale per row), everything else f16.
//! It runs on the CPU: about 10 ms per filter on a laptop.

use crate::{AiError, Result};
use candle_core::{DType, Device, IndexOp, Module, Tensor};
use candle_nn::{LayerNorm, Linear};
use std::collections::HashMap;
use std::path::Path;
use tokenizers::{Tokenizer, TruncationDirection, TruncationParams, TruncationStrategy};

const MAX_TOKENS: usize = 320;

struct Layer {
    q: Linear,
    k: Linear,
    v: Linear,
    attn_out: Linear,
    attn_norm: LayerNorm,
    up: Linear,
    down: Linear,
    out_norm: LayerNorm,
}

pub struct Sorter {
    tokenizer: Tokenizer,
    words_q: Tensor,
    words_scale: Tensor,
    positions: Tensor,
    token_type: Tensor,
    embed_norm: LayerNorm,
    layers: Vec<Layer>,
    head: Linear,
    heads: usize,
    hidden: usize,
    device: Device,
}

fn take(t: &mut HashMap<String, Tensor>, name: &str) -> Result<Tensor> {
    let tensor = t.remove(name).ok_or_else(|| AiError::Invalid(format!("sorter file is missing {name}")))?;
    Ok(tensor.to_dtype(DType::F32)?)
}

fn linear(t: &mut HashMap<String, Tensor>, prefix: &str) -> Result<Linear> {
    Ok(Linear::new(take(t, &format!("{prefix}.weight"))?, Some(take(t, &format!("{prefix}.bias"))?)))
}

fn norm(t: &mut HashMap<String, Tensor>, prefix: &str, eps: f64) -> Result<LayerNorm> {
    Ok(LayerNorm::new(take(t, &format!("{prefix}.weight"))?, take(t, &format!("{prefix}.bias"))?, eps))
}

impl Sorter {
    pub fn load(weights: &Path, tokenizer: &Path) -> Result<Self> {
        let device = Device::Cpu;
        let data = std::fs::read(weights)?;
        let meta = safetensors::SafeTensors::read_metadata(&data)
            .map_err(|e| AiError::Invalid(format!("sorter file: {e}")))?
            .1
            .metadata()
            .clone()
            .unwrap_or_default();
        if meta.get("format").map(String::as_str) != Some("email-sorter-1") {
            return Err(AiError::Invalid("not an Email sorter file".into()));
        }
        let num = |k: &str| -> Result<usize> {
            meta.get(k).and_then(|v| v.parse().ok()).ok_or_else(|| AiError::Invalid(format!("sorter file: {k}")))
        };
        let (hidden, layers_n, heads) = (num("hidden_size")?, num("num_hidden_layers")?, num("num_attention_heads")?);
        let eps: f64 = meta.get("layer_norm_eps").and_then(|v| v.parse().ok()).unwrap_or(1e-12);
        let mut t = candle_core::safetensors::load_buffer(&data, &device)?;
        drop(data);

        let words_q = t.remove("embeddings.word_embeddings.q").ok_or_else(|| AiError::Invalid("sorter file: embeddings".into()))?;
        let words_scale = take(&mut t, "embeddings.word_embeddings.scale")?;
        let positions = take(&mut t, "embeddings.position_embeddings.weight")?;
        let token_type = take(&mut t, "embeddings.token_type_embeddings.weight")?.i(0)?;
        let embed_norm = norm(&mut t, "embeddings.LayerNorm", eps)?;
        let mut layers = Vec::with_capacity(layers_n);
        for i in 0..layers_n {
            let p = format!("encoder.layer.{i}");
            layers.push(Layer {
                q: linear(&mut t, &format!("{p}.attention.self.query"))?,
                k: linear(&mut t, &format!("{p}.attention.self.key"))?,
                v: linear(&mut t, &format!("{p}.attention.self.value"))?,
                attn_out: linear(&mut t, &format!("{p}.attention.output.dense"))?,
                attn_norm: norm(&mut t, &format!("{p}.attention.output.LayerNorm"), eps)?,
                up: linear(&mut t, &format!("{p}.intermediate.dense"))?,
                down: linear(&mut t, &format!("{p}.output.dense"))?,
                out_norm: norm(&mut t, &format!("{p}.output.LayerNorm"), eps)?,
            });
        }
        let head = linear(&mut t, "head")?;

        let mut tokenizer = Tokenizer::from_file(tokenizer).map_err(|e| AiError::Tokenizer(e.to_string()))?;
        tokenizer
            .with_truncation(Some(TruncationParams {
                max_length: MAX_TOKENS,
                strategy: TruncationStrategy::OnlySecond,
                direction: TruncationDirection::Right,
                stride: 0,
            }))
            .map_err(|e| AiError::Tokenizer(e.to_string()))?;
        tokenizer.with_padding(None);
        Ok(Self { tokenizer, words_q, words_scale, positions, token_type, embed_norm, layers, head, heads, hidden, device })
    }

    /// Probability, for each description, that `email` belongs there.
    ///
    /// All pairs run as one packed batch: the dense layers see every token
    /// at once, attention runs per pair, so there is no padding.
    pub fn score(&self, descriptions: &[String], email: &str) -> Result<Vec<f32>> {
        if descriptions.is_empty() {
            return Ok(Vec::new());
        }
        let passage = format!("passage: {email}");
        let mut ids = Vec::new();
        let mut pos = Vec::new();
        let mut spans = Vec::new();
        for d in descriptions {
            let enc = self
                .tokenizer
                .encode((format!("query: {d}"), passage.clone()), true)
                .map_err(|e| AiError::Tokenizer(e.to_string()))?;
            spans.push((ids.len(), enc.get_ids().len()));
            pos.extend(0..enc.get_ids().len() as u32);
            ids.extend_from_slice(enc.get_ids());
        }
        let ids_t = Tensor::new(ids.as_slice(), &self.device)?;
        let pos_t = Tensor::new(pos.as_slice(), &self.device)?;
        // int8 row lookup, then dequantize.
        let rows = self.words_q.index_select(&ids_t, 0)?.to_dtype(DType::F32)?;
        let scale = self.words_scale.index_select(&ids_t, 0)?.unsqueeze(1)?;
        let words = (rows - 128.0)?.broadcast_mul(&scale)?;
        let x = (words + self.positions.index_select(&pos_t, 0)?)?.broadcast_add(&self.token_type)?;
        let mut x = self.embed_norm.forward(&x)?; // [tokens, hidden]

        let hd = self.hidden / self.heads;
        let scale = 1.0 / (hd as f64).sqrt();
        for l in &self.layers {
            let q = l.q.forward(&x)?;
            let k = l.k.forward(&x)?;
            let v = l.v.forward(&x)?;
            let mut ctx = Vec::with_capacity(spans.len());
            for &(start, n) in &spans {
                let split = |t: &Tensor| -> Result<Tensor> {
                    Ok(t.narrow(0, start, n)?.reshape((n, self.heads, hd))?.transpose(0, 1)?.contiguous()?)
                };
                let (q, k, v) = (split(&q)?, split(&k)?, split(&v)?);
                let att = candle_nn::ops::softmax_last_dim(&(q.matmul(&k.t()?)? * scale)?)?;
                ctx.push(att.matmul(&v)?.transpose(0, 1)?.reshape((n, self.hidden))?);
            }
            let ctx = Tensor::cat(&ctx, 0)?;
            x = l.attn_norm.forward(&(l.attn_out.forward(&ctx)? + &x)?)?;
            let h = l.up.forward(&x)?.gelu_erf()?;
            x = l.out_norm.forward(&(l.down.forward(&h)? + &x)?)?;
        }
        let pooled: Vec<Tensor> = spans.iter().map(|&(s, n)| x.narrow(0, s, n)?.mean(0)).collect::<std::result::Result<_, _>>()?;
        let logits = self.head.forward(&Tensor::stack(&pooled, 0)?)?.flatten_all()?.to_vec1::<f32>()?;
        Ok(logits.into_iter().map(|l| 1.0 / (1.0 + (-l).exp())).collect())
    }
}
