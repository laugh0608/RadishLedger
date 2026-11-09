/// 两个 bridge 候选调用同一 Rust 合成实验，结果是版本化 UTF-8 JSON。
pub fn run_probe(directory: String) -> String {
    radishledger_core::probe::run_json(&directory)
}

/// 普通 JSON 只含非秘密结果；秘密字节走独立有界字段，不可输出调试日志。
pub struct BootstrapReply {
    pub status_json: String,
    pub secret: Vec<u8>,
}

pub fn run_bootstrap(request_json: String, secret: Vec<u8>) -> BootstrapReply {
    let secret = zeroize::Zeroizing::new(secret);
    let result = std::panic::catch_unwind(|| {
        if request_json.is_empty() || request_json.len() > 4096 || secret.len() > 1104 {
            return Err(radishledger_core::CoreError::new(
                "invalid_argument",
                "bootstrap 请求长度无效",
            ));
        }
        let request = serde_json::from_str(&request_json).map_err(|_| {
            radishledger_core::CoreError::new("invalid_request", "bootstrap 请求格式无效")
        })?;
        radishledger_core::bootstrap::run(request, &secret)
    })
    .unwrap_or_else(|_| {
        Err(radishledger_core::CoreError::new(
            "internal_error",
            "核心调用异常",
        ))
    });
    match result {
        Ok(reply) => BootstrapReply {
            status_json: serde_json::json!({"ok":true,"report":reply.report}).to_string(),
            // FRB 转移此 Vec；Dart 接收后负责清理，codec 内部副本不能保证擦除。
            secret: reply.secret.to_vec(),
        },
        Err(error) => BootstrapReply {
            status_json: serde_json::json!({"ok":false,"error":error}).to_string(),
            secret: Vec::new(),
        },
    }
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
