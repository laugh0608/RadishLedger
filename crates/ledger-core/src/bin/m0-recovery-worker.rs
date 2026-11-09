//! 宿主跨进程实验入口，只从 stdin 读取有界的合成请求，不接收秘密参数。
use std::io::Read;
fn main() {
    let mut input = String::new();
    std::io::stdin()
        .take(4097)
        .read_to_string(&mut input)
        .expect("request stdin");
    assert!(input.len() <= 4096, "request bound");
    let request = serde_json::from_str(&input).expect("synthetic request");
    let result = match radishledger_core::validation::run(request) {
        Ok(report) => serde_json::json!({"ok":true,"report":report}),
        Err(error) => serde_json::json!({"ok":false,"error":error}),
    };
    println!("{result}");
}
