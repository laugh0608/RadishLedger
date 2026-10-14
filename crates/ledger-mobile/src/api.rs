/// 两个 bridge 候选调用同一 Rust 合成实验，结果是版本化 UTF-8 JSON。
pub fn run_probe(directory: String) -> String {
    radishledger_core::probe::run_json(&directory)
}

/// M0 故障实验专用，不接受生产账库或生产解锁材料。
pub fn run_validation(request_json: String) -> String {
    let response = std::panic::catch_unwind(|| {
        if request_json.len() > 4096 {
            return serde_json::json!({"ok":false,"error":{"code":"invalid_argument","message":"实验请求过长"}});
        }
        let request = match serde_json::from_str::<radishledger_core::validation::Request>(&request_json) {
            Ok(request) => request,
            Err(_) => return serde_json::json!({"ok":false,"error":{"code":"invalid_request","message":"实验请求格式无效"}}),
        };
        match radishledger_core::validation::run(request) {
            Ok(report) => serde_json::json!({"ok":true,"report":report}),
            Err(error) => serde_json::json!({"ok":false,"error":error}),
        }
    }).unwrap_or_else(|_| serde_json::json!({"ok":false,"error":{"code":"internal_error","message":"核心调用异常"}}));
    response.to_string()
}
