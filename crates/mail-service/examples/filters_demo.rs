//! Sort the launch demo mailbox with the real model, through the JSON API.
//!
//! Downloads the model first if it is missing.
//!
//! EMAIL_MODELS_DIR=<models dir> cargo run --release -p mail-service --example filters_demo -- <store.sqlite>
use mail_service::MailService;
use serde_json::{Value, json};

fn call(service: &MailService, request: Value) -> Value {
    service.request_json(&request.to_string()).unwrap_or_else(|e| panic!("{request}: {e}"))
}

fn main() {
    let path = std::env::args().nth(1).expect("store path");
    let service = MailService::open(&path).unwrap();
    call(&service, json!({"op":"seed_demo","set":"launch"}));
    let status = call(&service, json!({"op":"filters_setup"}));
    if status["model"]["installed"] != true {
        let started = std::time::Instant::now();
        let result = call(&service, json!({"op":"filters_download_model"}));
        eprintln!("download: {result} in {:?}", started.elapsed());
    }
    let names: std::collections::HashMap<String, String> = status["filters"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| (f["id"].as_str().unwrap().to_owned(), f["name"].as_str().unwrap().to_owned()))
        .collect();
    let started = std::time::Instant::now();
    loop {
        let result = call(&service, json!({"op":"filters_wake","budget_ms":60000}));
        eprintln!("moved {} remaining {} error {}", result["moved"], result["remaining"], result["error"]);
        if result["remaining"].as_u64() == Some(0) || !result["error"].is_null() {
            break;
        }
    }
    eprintln!("sorted in {:?}", started.elapsed());
    for (id, name) in &names {
        let threads = call(&service, json!({"op":"list_filter_threads","filter_id":id,"limit":200}));
        println!("\n== {name} ({})", threads.as_array().unwrap().len());
        for t in threads.as_array().unwrap() {
            println!("  {:<34} {:<50} | {}", t["sender"].as_str().unwrap().chars().take(34).collect::<String>(), t["subject"].as_str().unwrap().chars().take(50).collect::<String>(), t["filter"]["reason"].as_str().unwrap_or(""));
        }
    }
    let inbox = call(&service, json!({"op":"list_threads","limit":200}));
    println!("\n== Inbox ({})", inbox.as_array().unwrap().len());
    for t in inbox.as_array().unwrap() {
        println!("  {:<34} {}", t["sender"].as_str().unwrap().chars().take(34).collect::<String>(), t["subject"].as_str().unwrap().chars().take(60).collect::<String>());
    }
}
