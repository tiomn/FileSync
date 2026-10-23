//! 数据模型与持久化（config.json / history.json）

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::util::{data_dir, now_iso, log_line};

/// 内置默认排除规则
pub fn default_excludes() -> Vec<String> {
    [
        "*.tmp", "~$*", ".git", "node_modules", "Thumbs.db", "desktop.ini", "*.crdownload",
        "*.part", "*.swp",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect()
}

/// 当前工作配置
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CurrentConfig {
    pub source: String,
    pub target: String,
    /// incremental | mirror
    pub mode: String,
    /// auto | manual
    pub trigger: String,
    pub debounce_ms: u64,
    pub excludes: Vec<String>,
    pub strict_hash: bool,
    /// 整体替换删除时是否进回收站
    pub mirror_delete_to_recycle_bin: bool,
    /// 监听目录缺失时的重试间隔（秒）
    #[serde(default = "default_retry_interval")]
    pub retry_interval_secs: u64,
    /// 监听目录缺失时的最大重试次数（0 = 不限）
    #[serde(default = "default_retry_max")]
    pub retry_max: u64,
    /// 监听目录缺失时的最长等待时间（秒，0 = 不限）
    #[serde(default = "default_retry_timeout")]
    pub retry_timeout_secs: u64,
}

pub fn default_retry_interval() -> u64 {
    10
}
pub fn default_retry_max() -> u64 {
    30
}
pub fn default_retry_timeout() -> u64 {
    300
}

impl Default for CurrentConfig {
    fn default() -> Self {
        Self {
            source: String::new(),
            target: String::new(),
            mode: "incremental".into(),
            trigger: "auto".into(),
            debounce_ms: 1000,
            excludes: default_excludes(),
            strict_hash: false,
            mirror_delete_to_recycle_bin: true,
            retry_interval_secs: default_retry_interval(),
            retry_max: default_retry_max(),
            retry_timeout_secs: default_retry_timeout(),
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct NotifyConfig {
    /// 可同步通知：检测到变更但未同步时提醒
    pub on_pending: bool,
    /// 同步完成通知
    pub on_complete: bool,
    /// 出错通知
    pub on_error: bool,
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            on_pending: true,
            on_complete: true,
            on_error: true,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct AppConfig {
    pub minimize_to_tray: bool,
    pub close_to_tray: bool,
    pub auto_start: bool,
    /// light | dark | system
    pub theme: String,
    pub max_history: usize,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            minimize_to_tray: true,
            close_to_tray: true,
            auto_start: false,
            theme: "system".into(),
            max_history: 50,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    #[serde(default)]
    pub current: CurrentConfig,
    #[serde(default)]
    pub notify: NotifyConfig,
    #[serde(default)]
    pub app: AppConfig,
}

/// 上次同步结果
#[derive(Serialize, Deserialize, Clone, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct LastResult {
    pub ok: bool,
    pub added: u64,
    pub updated: u64,
    pub deleted: u64,
    pub skipped: u64,
    pub failed: u64,
    pub ms: u64,
}

/// 历史方案
#[derive(Serialize, Deserialize, Clone, Debug)]
#[serde(rename_all = "camelCase")]
pub struct Plan {
    pub id: String,
    pub name: String,
    pub source: String,
    pub target: String,
    pub mode: String,
    pub trigger: String,
    #[serde(default)]
    pub last_run_at: Option<String>,
    #[serde(default)]
    pub last_result: Option<LastResult>,
    #[serde(default)]
    pub use_count: u64,
}

/// 统一持久层
pub struct Store {
    pub config: Config,
    pub plans: Vec<Plan>,
}

impl Store {
    fn config_path() -> PathBuf {
        data_dir().join("config.json")
    }

    fn history_path() -> PathBuf {
        data_dir().join("history.json")
    }

    pub fn load() -> Self {
        let config = Self::read_json::<Config>(Self::config_path()).unwrap_or_default();
        let plans = Self::read_json::<Vec<Plan>>(Self::history_path()).unwrap_or_default();

        Self { config, plans }
    }

    /// 读取 JSON 文件，容忍 UTF-8 BOM（记事本等编辑器会写入 BOM，
    /// 而 serde_json 不接受前导 BOM，直接解析会静默回退到默认配置）
    fn read_json<T: serde::de::DeserializeOwned>(path: PathBuf) -> Option<T> {
        let text = fs::read_to_string(&path).ok()?;
        let text = text.trim_start_matches('\u{feff}');
        serde_json::from_str::<T>(text).ok()
    }

    pub fn save_config(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.config) {
            if let Err(e) = fs::write(Self::config_path(), s) {
                log_line("config", &format!("写入 config.json 失败: {e}"));
            }
        }
    }

    pub fn save_history(&self) {
        if let Ok(s) = serde_json::to_string_pretty(&self.plans) {
            if let Err(e) = fs::write(Self::history_path(), s) {
                log_line("history", &format!("写入 history.json 失败: {e}"));
            }
        }
    }

    /// 保存/更新当前配置对应的方案，并记录同步结果
    pub fn upsert_plan_from_current(&mut self, result: Option<LastResult>) -> String {
        let cur = self.config.current.clone();
        let idx = self
            .plans
            .iter()
            .position(|p| p.source == cur.source && p.target == cur.target);
        let id = if let Some(i) = idx {
            let p = &mut self.plans[i];
            p.mode = cur.mode.clone();
            p.trigger = cur.trigger.clone();
            p.use_count += 1;
            if let Some(r) = result {
                p.last_run_at = Some(now_iso());
                p.last_result = Some(r);
            }
            p.id.clone()
        } else {
            let id = format!("h_{}", chrono::Local::now().timestamp_millis());
            let name = auto_plan_name(&cur.source, &cur.target);
            self.plans.insert(
                0,
                Plan {
                    id: id.clone(),
                    name,
                    source: cur.source.clone(),
                    target: cur.target.clone(),
                    mode: cur.mode.clone(),
                    trigger: cur.trigger.clone(),
                    last_run_at: result.as_ref().map(|_| now_iso()),
                    last_result: result,
                    use_count: 1,
                },
            );
            id
        };
        self.trim_history();
        self.save_history();
        id
    }

    pub fn trim_history(&mut self) {
        let max = self.config.app.max_history.max(5);
        if self.plans.len() > max {
            self.plans.truncate(max);
        }
    }
}

/// 由目录自动生成方案名
pub fn auto_plan_name(source: &str, target: &str) -> String {
    let last = |p: &str| {
        std::path::Path::new(p)
            .file_name()
            .map(|s| s.to_string_lossy().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| p.to_string())
    };
    format!("{} → {}", last(source), last(target))
}
