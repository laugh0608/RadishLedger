/// 两个 bridge 候选调用同一 Rust 合成实验，结果是版本化 UTF-8 JSON。
pub fn run_probe(directory: String) -> String {
    radishledger_core::probe::run_json(&directory)
}
