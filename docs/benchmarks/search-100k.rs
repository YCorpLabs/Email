use mail_core::{MailStore, Account, Provider};
use std::time::Instant;
fn main() {
    let count: usize=std::env::args().nth(1).unwrap_or("10000".into()).parse().unwrap();
    let path=std::env::temp_dir().join(format!("email-search-bench-{}.sqlite",std::process::id()));
    let store=MailStore::open(&path).unwrap();
    store.seed_demo().unwrap();
    for a in 0..4 {
        store.upsert_account(&Account{id:format!("bench:{a}"),provider:Provider::Gmail,email:format!("account{a}@example.com"),display_name:format!("Account {a}")}).unwrap();
    }
    let mut m=store.get_message("demo:1").unwrap().unwrap();
    let start=Instant::now();
    for i in 0..count {
        m.id=format!("bench:{i}");m.thread_id=m.id.clone();m.account_id=format!("bench:{}",i%4);
        m.sender=if i%7==0 {"Billing <billing@example.org>"}else{"Newsletter <news@example.com>"}.into();
        m.subject=if i%7==0 {"Invoice for November"}else{"Weekly product newsletter"}.into();
        m.body_plain="Cached synthetic text with invoice project and newsletter terms. ".repeat(20);
        m.received_at=1_790_000_000_000+i as i64;m.is_read=i%3==0;
        store.upsert_message(&m).unwrap();
    }
    println!("corpus={count},accounts=4,seed_ms={:.1},local SQLite only",start.elapsed().as_secs_f64()*1000.);
    for query in ["", "invoice", "newsletter", "from:billing@example.org", "is:unread subject:invoice", "to:alex@example.com", "nonexistentterm"] {
        let mut samples=vec![];let mut hits=0;
        for _ in 0..31 {let now=Instant::now();hits=store.search_scoped(query,100,0,&Default::default()).unwrap().len();samples.push(now.elapsed().as_secs_f64()*1000.);}
        let first=samples[0];samples.remove(0);samples.sort_by(f64::total_cmp);
        println!("query={query:?},hits={hits},first_ms={first:.3},warm_p50_ms={:.3},warm_p95_ms={:.3}",samples[15],samples[28]);
    }
    drop(store);let _=std::fs::remove_file(&path);let _=std::fs::remove_file(path.with_extension("sqlite-wal"));let _=std::fs::remove_file(path.with_extension("sqlite-shm"));
}
