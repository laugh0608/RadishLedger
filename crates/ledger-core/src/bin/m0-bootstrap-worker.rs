//! 宿主强杀实验：秘密只经 stdin/stdout 管道传递，不接收 argv 或环境变量材料。
use radishledger_core::{CoreError, bootstrap};
use std::io::{Read, Write};
use zeroize::Zeroizing;

fn main() {
    let result = std::panic::catch_unwind(|| -> radishledger_core::Result<bootstrap::Reply> {
        let mut header = [0; 8];
        std::io::stdin()
            .read_exact(&mut header)
            .map_err(|_| CoreError::new("transport_error", "实验管道头不完整"))?;
        let metadata_len = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
        let secret_len = u32::from_be_bytes(header[4..].try_into().unwrap()) as usize;
        if metadata_len == 0 || metadata_len > 4096 || secret_len > 1104 {
            return Err(CoreError::new("invalid_argument", "实验输入过长"));
        }
        let mut metadata = vec![0; metadata_len];
        let mut secret = Zeroizing::new(vec![0; secret_len]);
        std::io::stdin()
            .read_exact(&mut metadata)
            .and_then(|_| std::io::stdin().read_exact(&mut secret))
            .map_err(|_| CoreError::new("transport_error", "实验管道内容不完整"))?;
        let request = serde_json::from_slice(&metadata)
            .map_err(|_| CoreError::new("invalid_request", "实验请求无效"))?;
        bootstrap::run(request, &secret)
    })
    .unwrap_or_else(|_| Err(CoreError::new("internal_error", "实验核心异常")));
    let (metadata, secret) = match result {
        Ok(reply) => (
            serde_json::json!({"ok":true,"report":reply.report}).to_string(),
            reply.secret,
        ),
        Err(error) => (
            serde_json::json!({"ok":false,"error":error}).to_string(),
            Zeroizing::new(Vec::new()),
        ),
    };
    let mut stdout = std::io::stdout().lock();
    let result = stdout
        .write_all(&(metadata.len() as u32).to_be_bytes())
        .and_then(|_| stdout.write_all(&(secret.len() as u32).to_be_bytes()))
        .and_then(|_| stdout.write_all(metadata.as_bytes()))
        .and_then(|_| stdout.write_all(&secret))
        .and_then(|_| stdout.flush());
    if result.is_err() {
        std::process::exit(1);
    }
}
