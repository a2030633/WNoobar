#![allow(dead_code)]
use std::time::Instant;

use crate::core::swarm::SwarmSummary;

#[derive(Clone, Debug, PartialEq)]
pub struct ActiveSessionItem {
    pub session_id: String,
    pub project_name: String,
    pub role_type: String,
    pub task_summary: String,
    pub agent_name: String,
    pub step_index: u64,
    pub start_time: Instant,
    pub last_update_time: Instant,
    pub swarm: Option<SwarmSummary>,
}

pub struct OdometerCarousel {
    pub active_index: usize,
    pub prev_index: usize,
    pub dwell_timer: f32,
    pub is_rolling: bool,
    pub roll_progress: f32,
    pub roll_duration: f32,
    pub landing_bounce_time: f32,
}

impl OdometerCarousel {
    pub fn new() -> Self {
        Self {
            active_index: 0,
            prev_index: 0,
            dwell_timer: 0.0,
            is_rolling: false,
            roll_progress: 0.0,
            roll_duration: 0.6, // 600ms 黄金阻尼翻滚周期
            landing_bounce_time: 0.0,
        }
    }

    /// 推进物理时间，返回是否处于动态动画帧中（需驱动渲染）
    pub fn update(&mut self, dt: f32, session_count: usize) -> bool {
        if session_count < 2 {
            self.is_rolling = false;
            self.dwell_timer = 0.0;
            self.roll_progress = 0.0;
            self.landing_bounce_time = 0.0;
            self.active_index = 0;
            return false;
        }

        // 会话数量缩减防御
        if self.active_index >= session_count {
            self.active_index = 0;
        }
        if self.prev_index >= session_count {
            self.prev_index = 0;
        }

        let mut needs_render = false;

        if self.is_rolling {
            needs_render = true;
            self.roll_progress += dt / self.roll_duration;
            if self.roll_progress >= 1.0 {
                self.roll_progress = 1.0;
                self.is_rolling = false;
                self.landing_bounce_time = 0.22; // 220ms 落地微弹
                self.dwell_timer = 0.0;
            }
        } else if self.landing_bounce_time > 0.0 {
            needs_render = true;
            self.landing_bounce_time = (self.landing_bounce_time - dt).max(0.0);
        } else {
            // 正常驻留阶段（3.2 秒）
            self.dwell_timer += dt;
            if self.dwell_timer >= 3.2 {
                self.trigger_next(session_count);
                needs_render = true;
            }
        }

        needs_render
    }

    /// 手动或定时触发换页翻滚
    pub fn trigger_next(&mut self, session_count: usize) {
        if session_count >= 2 {
            self.is_rolling = true;
            self.roll_progress = 0.0;
            self.prev_index = self.active_index;
            self.active_index = (self.active_index + 1) % session_count;
            self.dwell_timer = 0.0;
            self.landing_bounce_time = 0.0;
        }
    }

    /// Cubic EaseOut 缓动进度因子 (0.0 -> 1.0)
    pub fn ease_progress(&self) -> f32 {
        let t = self.roll_progress.clamp(0.0, 1.0);
        1.0 - (1.0 - t).powi(3)
    }

    /// 计算前后两行内容的垂直 translateY 偏移量
    /// 返回: (offset_y_prev, offset_y_curr)
    pub fn offsets(&self, row_height: f32) -> (f32, f32) {
        if self.is_rolling {
            let ease_t = self.ease_progress();
            let prev_offset = -ease_t * row_height;
            let curr_offset = (1.0 - ease_t) * row_height;
            (prev_offset, curr_offset)
        } else {
            (0.0, 0.0)
        }
    }

    /// 落地微弹强调比例 (1.0 -> 1.04 -> 1.0)
    pub fn landing_scale(&self) -> f32 {
        if self.landing_bounce_time > 0.0 {
            let p = 1.0 - (self.landing_bounce_time / 0.22).clamp(0.0, 1.0);
            let bounce = (p * std::f32::consts::PI).sin();
            1.0 + 0.04 * bounce
        } else {
            1.0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_odometer_dwell_to_rolling() {
        let mut odometer = OdometerCarousel::new();
        // 会话数小于2时，静止不轮播
        assert!(!odometer.update(1.0, 1));
        assert_eq!(odometer.active_index, 0);

        // 会话数达到2
        // 驻留 3.0s，仍未触发
        assert!(!odometer.update(3.0, 2));
        assert!(!odometer.is_rolling);

        // 累计超过 3.2s，触发翻滚
        assert!(odometer.update(0.3, 2));
        assert!(odometer.is_rolling);
        assert_eq!(odometer.prev_index, 0);
        assert_eq!(odometer.active_index, 1);
        assert_eq!(odometer.roll_progress, 0.0);
    }

    #[test]
    fn test_odometer_ease_out_curve() {
        let mut odometer = OdometerCarousel::new();
        odometer.is_rolling = true;
        odometer.roll_progress = 0.5;

        let ease = odometer.ease_progress();
        // EaseOut 曲线在 0.5 处必须大于线性 0.5 (前快)
        assert!(ease > 0.5);
        assert!((ease - 0.875).abs() < 1e-4);

        let (prev_y, curr_y) = odometer.offsets(20.0);
        assert_eq!(prev_y, -17.5);
        assert_eq!(curr_y, 2.5);
    }

    #[test]
    fn test_odometer_landing_scale_bounce() {
        let mut odometer = OdometerCarousel::new();
        odometer.landing_bounce_time = 0.11; // 处于 0.22s 的中点 (正弦峰值)
        let scale = odometer.landing_scale();
        assert!(scale > 1.035 && scale <= 1.0401);

        odometer.landing_bounce_time = 0.0;
        assert_eq!(odometer.landing_scale(), 1.0);
    }
}
