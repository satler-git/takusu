//! Workload configuration for the planner.

/// 1 日あたりの作業負荷設定。
///
/// ユーザーの「1 日にどれくらいのタスクを入れたいか」を表す。
/// デフォルトでは内部で決定された値を使い、詳細を指定したい場合だけ
/// `PlannerConfig` 経由で上書きする。
///
/// 不変条件: `comfortable_slots_per_day <= maximum_slots_per_day`。
#[derive(Debug, Clone, Copy)]
pub struct WorkloadConfig {
    /// 快適な 1 日あたりの作業スロット数（5 分単位）。
    /// この値を超えると緩やかなペナルティがかかる。
    comfortable_slots_per_day: i64,
    /// 1 日あたりの作業スロット数の上限（5 分単位）。
    /// この値を超えると強いペナルティがかかる。
    maximum_slots_per_day: i64,
}

impl WorkloadConfig {
    /// 負荷評価を無効化する。
    pub fn disabled() -> Self {
        Self::new(0, 0)
    }

    /// 任意の閾値を指定する。
    ///
    /// `comfortable_slots_per_day <= maximum_slots_per_day` でなければならない。
    pub fn new(comfortable_slots_per_day: i64, maximum_slots_per_day: i64) -> Self {
        assert!(
            comfortable_slots_per_day <= maximum_slots_per_day,
            "WorkloadConfig::new: comfortable ({comfortable_slots_per_day}) \
             must be <= maximum ({maximum_slots_per_day})"
        );
        Self {
            comfortable_slots_per_day,
            maximum_slots_per_day,
        }
    }

    /// 快適な 1 日あたりの作業スロット数（5 分単位）。
    pub fn comfortable_slots_per_day(&self) -> i64 {
        self.comfortable_slots_per_day
    }

    /// 1 日あたりの作業スロット数の上限（5 分単位）。
    pub fn maximum_slots_per_day(&self) -> i64 {
        self.maximum_slots_per_day
    }
}

impl Default for WorkloadConfig {
    /// デフォルト設定: 快適 8 時間（96 スロット）、上限 12 時間（144 スロット）。
    fn default() -> Self {
        Self::new(96, 144)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workload_config_new_preserves_values() {
        let wc = WorkloadConfig::new(48, 96);
        assert_eq!(wc.comfortable_slots_per_day(), 48);
        assert_eq!(wc.maximum_slots_per_day(), 96);
    }

    #[test]
    fn workload_config_default_values() {
        let wc = WorkloadConfig::default();
        assert_eq!(wc.comfortable_slots_per_day(), 96);
        assert_eq!(wc.maximum_slots_per_day(), 144);
        assert!(
            wc.comfortable_slots_per_day() <= wc.maximum_slots_per_day(),
            "default must satisfy comfortable <= maximum"
        );
    }

    #[test]
    fn workload_config_disabled_values() {
        let wc = WorkloadConfig::disabled();
        assert_eq!(wc.comfortable_slots_per_day(), 0);
        assert_eq!(wc.maximum_slots_per_day(), 0);
    }

    #[test]
    #[should_panic(expected = "comfortable (100)")]
    fn workload_config_new_rejects_comfortable_gt_maximum() {
        let _ = WorkloadConfig::new(100, 50);
    }

    #[test]
    fn workload_config_new_allows_comfortable_eq_maximum() {
        let wc = WorkloadConfig::new(80, 80);
        assert_eq!(wc.comfortable_slots_per_day(), 80);
        assert_eq!(wc.maximum_slots_per_day(), 80);
    }

    #[test]
    fn workload_config_new_valid_when_comfortable_lt_maximum() {
        let wc = WorkloadConfig::new(48, 96);
        assert_eq!(wc.comfortable_slots_per_day(), 48);
        assert_eq!(wc.maximum_slots_per_day(), 96);
    }
}
