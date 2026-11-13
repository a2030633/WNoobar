#![allow(dead_code)]
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::core::bus::NoiseFilterLevel;

static SAVE_SEQ: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum MonitorMode {
    #[default]
    Primary,
    FollowCursor,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct AppConfig {
    pub noise_level: NoiseFilterLevel,
    pub auto_start: bool,
    pub sound_enabled: bool,
    pub dnd_fullscreen: bool,
    pub monitor_mode: MonitorMode,
    pub start_pop_secs: f32,
    pub done_pop_secs: f32,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            noise_level: NoiseFilterLevel::Normal,
            auto_start: false,
            sound_enabled: false,
            dnd_fullscreen: true,
            monitor_mode: MonitorMode::Primary,
            start_pop_secs: 1.8,
            done_pop_secs: 3.5,
        }
    }
}

/// 线程安全的实时降噪等级共享状态（可在托盘/UI线程与后台监听器之间零锁热切换）
#[derive(Debug, Clone)]
pub struct SharedNoiseFilter {
    inner: Arc<AtomicU8>,
}

impl Default for SharedNoiseFilter {
    fn default() -> Self {
        Self::new(NoiseFilterLevel::Normal)
    }
}

impl SharedNoiseFilter {
    fn encode(level: NoiseFilterLevel) -> u8 {
        match level {
            NoiseFilterLevel::Silent => 0,
            NoiseFilterLevel::Normal => 1,
            NoiseFilterLevel::Verbose => 2,
        }
    }

    fn decode(raw: u8) -> NoiseFilterLevel {
        match raw {
            0 => NoiseFilterLevel::Silent,
            1 => NoiseFilterLevel::Normal,
            2 => NoiseFilterLevel::Verbose,
            _ => NoiseFilterLevel::Normal,
        }
    }

    pub fn new(level: NoiseFilterLevel) -> Self {
        Self {
            inner: Arc::new(AtomicU8::new(Self::encode(level))),
        }
    }

    pub fn get(&self) -> NoiseFilterLevel {
        Self::decode(self.inner.load(Ordering::SeqCst))
    }

    pub fn set(&self, level: NoiseFilterLevel) {
        self.inner.store(Self::encode(level), Ordering::SeqCst);
    }
}

impl AppConfig {
    /// 获取配置文件路径：优先 `%APPDATA%\WNoobar\config.json`，若无 `APPDATA` 则回退当前目录 `wnoobar.config.json`
    pub fn config_path() -> PathBuf {
        Self::config_path_with_appdata(std::env::var_os("APPDATA").as_deref())
    }

    fn config_path_with_appdata(appdata: Option<&OsStr>) -> PathBuf {
        if let Some(dir) = appdata {
            if !dir.is_empty() {
                return PathBuf::from(dir).join("WNoobar").join("config.json");
            }
        }
        PathBuf::from("wnoobar.config.json")
    }

    /// 从指定路径加载配置；文件不存在或 JSON 损坏时安全回退为 `Self::default()`
    pub fn load_from(path: &Path) -> Self {
        let Ok(raw) = fs::read_to_string(path) else {
            return Self::default();
        };
        let trimmed = raw.trim_start_matches('\u{feff}').trim();
        if trimmed.is_empty() {
            return Self::default();
        }
        serde_json::from_str(trimmed).unwrap_or_default()
    }

    /// 从默认配置路径加载配置
    pub fn load() -> Self {
        let path = Self::config_path();
        Self::load_from(&path)
    }

    /// 保存配置到指定路径：自动创建父目录，写入美化 JSON，支持临时文件写入后替换（并兼容直接覆盖）
    pub fn save_to(&self, path: &Path) -> Result<(), String> {
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent)
                    .map_err(|e| format!("Failed to create config directory {:?}: {e}", parent))?;
            }
        }

        let json = serde_json::to_string_pretty(self)
            .map_err(|e| format!("Failed to serialize AppConfig: {e}"))?;

        let seq = SAVE_SEQ.fetch_add(1, Ordering::Relaxed);
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("config.json");
        let tmp_name = format!(".{file_name}.{}.{seq}.tmp", std::process::id());
        let tmp_path = match path.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => parent.join(tmp_name),
            _ => PathBuf::from(tmp_name),
        };

        if let Err(write_err) = fs::write(&tmp_path, json.as_bytes()) {
            let _ = fs::remove_file(&tmp_path);
            return fs::write(path, json.as_bytes()).map_err(|direct_err| {
                format!(
                    "Failed to write temp config {:?} ({write_err}) and direct fallback {:?} ({direct_err})",
                    tmp_path, path
                )
            });
        }

        match fs::rename(&tmp_path, path) {
            Ok(()) => Ok(()),
            Err(rename_err) => {
                let direct_res = fs::write(path, json.as_bytes());
                let _ = fs::remove_file(&tmp_path);
                direct_res.map_err(|direct_err| {
                    format!(
                        "Failed to replace config {:?} via rename ({rename_err}) or direct write ({direct_err})",
                        path
                    )
                })
            }
        }
    }

    /// 保存配置到默认配置路径
    pub fn save(&self) -> Result<(), String> {
        let path = Self::config_path();
        self.save_to(&path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Barrier;
    use std::thread;

    fn make_temp_dir(label: &str) -> PathBuf {
        let seq = SAVE_SEQ.fetch_add(1, Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "wnoobar_config_test_{}_{}_{}",
            label,
            std::process::id(),
            seq
        ))
    }

    #[test]
    fn test_default_config() {
        let cfg = AppConfig::default();
        assert_eq!(cfg.noise_level, NoiseFilterLevel::Normal);
        assert!(!cfg.auto_start);
        assert!(!cfg.sound_enabled);
        assert!(cfg.dnd_fullscreen);
        assert_eq!(cfg.monitor_mode, MonitorMode::Primary);
        assert!((cfg.start_pop_secs - 1.8).abs() < f32::EPSILON);
        assert!((cfg.done_pop_secs - 3.5).abs() < f32::EPSILON);
        assert_eq!(MonitorMode::default(), MonitorMode::Primary);
        assert_eq!(NoiseFilterLevel::default(), NoiseFilterLevel::Normal);
    }

    #[test]
    fn test_partial_fields_deserialize_uses_defaults() {
        let partial_json = r#"{
            "sound_enabled": true,
            "monitor_mode": "FollowCursor"
        }"#;
        let cfg: AppConfig = serde_json::from_str(partial_json).expect("partial JSON should parse");
        assert_eq!(cfg.noise_level, NoiseFilterLevel::Normal);
        assert!(!cfg.auto_start);
        assert!(cfg.sound_enabled);
        assert!(cfg.dnd_fullscreen);
        assert_eq!(cfg.monitor_mode, MonitorMode::FollowCursor);
        assert!((cfg.start_pop_secs - 1.8).abs() < f32::EPSILON);
        assert!((cfg.done_pop_secs - 3.5).abs() < f32::EPSILON);

        let empty_obj: AppConfig = serde_json::from_str("{}").expect("empty JSON object should parse");
        assert_eq!(empty_obj, AppConfig::default());
    }

    #[test]
    fn test_corrupted_json_and_missing_file_fallback() {
        let dir = make_temp_dir("fallback");
        let missing = dir.join("does_not_exist.json");
        assert_eq!(AppConfig::load_from(&missing), AppConfig::default());

        fs::create_dir_all(&dir).expect("create temp dir");

        let bad_json_path = dir.join("corrupt.json");
        fs::write(&bad_json_path, "{ not valid json !!!").expect("write corrupt file");
        assert_eq!(AppConfig::load_from(&bad_json_path), AppConfig::default());

        let empty_file_path = dir.join("empty.json");
        fs::write(&empty_file_path, "   \r\n  ").expect("write empty file");
        assert_eq!(AppConfig::load_from(&empty_file_path), AppConfig::default());

        let wrong_type_path = dir.join("wrong_type.json");
        fs::write(&wrong_type_path, r#"{"dnd_fullscreen": "not-a-bool"}"#)
            .expect("write wrong type file");
        assert_eq!(AppConfig::load_from(&wrong_type_path), AppConfig::default());

        let bom_path = dir.join("bom.json");
        fs::write(&bom_path, "\u{feff}{\"auto_start\": true}").expect("write BOM file");
        let loaded_bom = AppConfig::load_from(&bom_path);
        assert!(loaded_bom.auto_start);
        assert!(loaded_bom.dnd_fullscreen);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = make_temp_dir("roundtrip");
        let nested_path = dir.join("nested").join("WNoobar").join("config.json");

        let custom = AppConfig {
            noise_level: NoiseFilterLevel::Verbose,
            auto_start: true,
            sound_enabled: true,
            dnd_fullscreen: false,
            monitor_mode: MonitorMode::FollowCursor,
            start_pop_secs: 2.25,
            done_pop_secs: 4.75,
        };

        custom.save_to(&nested_path).expect("save_to should succeed");
        assert!(nested_path.exists());

        let raw = fs::read_to_string(&nested_path).expect("read saved config");
        assert!(raw.contains('\n'), "saved JSON should be pretty-printed");

        let loaded = AppConfig::load_from(&nested_path);
        assert_eq!(loaded, custom);

        // 再次保存覆盖，确保无残留临时文件且更新生效
        let updated = AppConfig {
            noise_level: NoiseFilterLevel::Silent,
            ..custom
        };
        updated.save_to(&nested_path).expect("overwrite save_to should succeed");
        assert_eq!(AppConfig::load_from(&nested_path), updated);

        let leftovers: Vec<_> = fs::read_dir(nested_path.parent().unwrap())
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".tmp"))
            .collect();
        assert!(leftovers.is_empty(), "temp files should be cleaned up");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_config_path_resolution() {
        let with_appdata = AppConfig::config_path_with_appdata(Some(OsStr::new(r"C:\Users\Test\AppData\Roaming")));
        assert_eq!(
            with_appdata,
            PathBuf::from(r"C:\Users\Test\AppData\Roaming")
                .join("WNoobar")
                .join("config.json")
        );

        assert_eq!(
            AppConfig::config_path_with_appdata(None),
            PathBuf::from("wnoobar.config.json")
        );
        assert_eq!(
            AppConfig::config_path_with_appdata(Some(OsStr::new(""))),
            PathBuf::from("wnoobar.config.json")
        );
    }

    #[test]
    fn test_shared_noise_filter_atomic_and_cross_thread() {
        fn assert_clone_send_sync<T: Clone + Send + Sync>() {}
        assert_clone_send_sync::<SharedNoiseFilter>();

        let filter = SharedNoiseFilter::new(NoiseFilterLevel::Normal);
        assert_eq!(filter.get(), NoiseFilterLevel::Normal);

        filter.set(NoiseFilterLevel::Silent);
        assert_eq!(filter.get(), NoiseFilterLevel::Silent);

        filter.set(NoiseFilterLevel::Verbose);
        assert_eq!(filter.get(), NoiseFilterLevel::Verbose);

        assert_eq!(SharedNoiseFilter::default().get(), NoiseFilterLevel::Normal);

        // 验证跨线程写入与读取的即时可见性
        let writer_handle = filter.clone();
        let reader_handle = filter.clone();
        let step1 = Arc::new(Barrier::new(2));
        let step2 = Arc::new(Barrier::new(2));
        let step3 = Arc::new(Barrier::new(2));

        let s1 = step1.clone();
        let s2 = step2.clone();
        let s3 = step3.clone();

        let reader = thread::spawn(move || {
            s1.wait();
            assert_eq!(reader_handle.get(), NoiseFilterLevel::Silent);
            s2.wait();
            s3.wait();
            assert_eq!(reader_handle.get(), NoiseFilterLevel::Normal);
        });

        writer_handle.set(NoiseFilterLevel::Silent);
        step1.wait();
        step2.wait();
        writer_handle.set(NoiseFilterLevel::Normal);
        step3.wait();

        reader.join().expect("reader thread should finish cleanly");
        assert_eq!(filter.get(), NoiseFilterLevel::Normal);
    }
}
