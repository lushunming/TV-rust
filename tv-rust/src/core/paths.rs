//! 路径约定：与 composeApp (Kotlin) 保持一致，复用同一份 jar / 配置缓存。
//! Windows: %AppData%\TV-Multiplatform\cache\{data\cache\jar, data\cache\config, db, ...}

use std::path::PathBuf;

pub const APP_NAME: &str = "TV-Multiplatform";

pub fn cache_root() -> PathBuf {
    let base = dirs::data_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join(APP_NAME).join("cache")
}

/// 对应 Kotlin Paths.root() = userDataDir/data
pub fn data_root() -> PathBuf {
    cache_root().join("data")
}

/// 对应 Kotlin Paths.cache(path) = data/cache/<path>
fn shared_cache(name: &str) -> PathBuf {
    data_root().join("cache").join(name)
}

/// 爬虫 jar 缓存目录（与 Kotlin Paths.jar() 相同）
pub fn jar_cache() -> PathBuf {
    shared_cache("jar")
}

/// 配置缓存目录（与 Kotlin Paths.configCache() 相同）
pub fn config_cache() -> PathBuf {
    shared_cache("config")
}

/// Rust 侧自有数据（历史/设置），不与 Kotlin 的 Room DB 混用
pub fn rust_data() -> PathBuf {
    cache_root().join("rust")
}

/// 播放器输出日志（与 Kotlin Paths.playerLog() 相同）
pub fn player_log() -> PathBuf {
    data_root().join("playerLog.txt")
}

pub fn ensure_dir(p: &PathBuf) -> PathBuf {
    let _ = std::fs::create_dir_all(p);
    p.clone()
}
