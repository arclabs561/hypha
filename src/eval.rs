//! Rigorous evaluation framework inspired by Protocol Labs' Gossipsub v1.1 Report.
//!
//! Key metrics:
//! - Delivery Rate: % of messages reaching all intended recipients
//! - Delivery Latency: CDF, percentiles (p50, p90, p99, p999)
//! - Convergence Time: time until all nodes have consistent state
//! - Energy Efficiency: mAh consumed per successful message delivery
//! - Recovery Time: time to recover from fault injection

use std::time::{Duration, Instant};
use serde::{Deserialize, Serialize};

/// Collected during a single evaluation run
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalRun {
    pub scenario: String,
    pub node_count: usize,
    pub duration: Duration,
    pub delivery: DeliveryMetrics,
    pub energy: EnergyMetrics,
    pub consistency: ConsistencyMetrics,
    pub fault_events: Vec<FaultEvent>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DeliveryMetrics {
    pub messages_published: u64,
    /// Total node-deliveries (each node receiving a message counts as 1)
    pub messages_delivered: u64,
    /// Expected total deliveries (messages_published * node_count)
    pub expected_deliveries: u64,
    /// Latency samples in microseconds
    pub latencies_us: Vec<u64>,
}

impl DeliveryMetrics {
    /// True delivery rate: fraction of expected deliveries achieved
    pub fn delivery_rate(&self) -> f64 {
        if self.expected_deliveries == 0 {
            return 0.0;
        }
        (self.messages_delivered as f64 / self.expected_deliveries as f64).min(1.0)
    }

    /// Compute percentile from latency samples
    pub fn percentile(&self, p: f64) -> Option<Duration> {
        if self.latencies_us.is_empty() {
            return None;
        }
        let mut sorted = self.latencies_us.clone();
        sorted.sort_unstable();
        let idx = ((p / 100.0) * (sorted.len() - 1) as f64).round() as usize;
        Some(Duration::from_micros(sorted[idx]))
    }

    pub fn p50(&self) -> Option<Duration> { self.percentile(50.0) }
    pub fn p90(&self) -> Option<Duration> { self.percentile(90.0) }
    pub fn p99(&self) -> Option<Duration> { self.percentile(99.0) }
    pub fn p999(&self) -> Option<Duration> { self.percentile(99.9) }

    /// CDF: returns (latency_bucket_us, cumulative_fraction) pairs
    pub fn cdf(&self, buckets: usize) -> Vec<(u64, f64)> {
        if self.latencies_us.is_empty() {
            return vec![];
        }
        let mut sorted = self.latencies_us.clone();
        sorted.sort_unstable();
        let max = *sorted.last().unwrap();
        let step = (max / buckets as u64).max(1);
        
        let mut result = Vec::with_capacity(buckets);
        let n = sorted.len() as f64;
        
        for i in 0..buckets {
            let threshold = (i as u64 + 1) * step;
            let count = sorted.iter().filter(|&&x| x <= threshold).count();
            result.push((threshold, count as f64 / n));
        }
        result
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EnergyMetrics {
    /// Total mAh consumed across all nodes
    pub total_mah_consumed: f32,
    /// mAh consumed per successfully delivered message
    pub mah_per_delivery: f32,
    /// Number of nodes that exhausted energy
    pub nodes_exhausted: usize,
    /// Energy score distribution at end (sorted)
    pub final_energy_scores: Vec<f32>,
}

impl EnergyMetrics {
    pub fn efficiency_ratio(&self) -> f32 {
        if self.total_mah_consumed == 0.0 {
            return 0.0;
        }
        // Higher is better: deliveries per mAh
        1.0 / self.mah_per_delivery.max(0.001)
    }

    /// Gini coefficient of energy distribution (0 = equal, 1 = maximally unequal)
    pub fn energy_gini(&self) -> f32 {
        if self.final_energy_scores.is_empty() {
            return 0.0;
        }
        let n = self.final_energy_scores.len() as f32;
        let mean = self.final_energy_scores.iter().sum::<f32>() / n;
        if mean == 0.0 {
            return 0.0;
        }
        
        let mut sum_diff = 0.0;
        for &xi in &self.final_energy_scores {
            for &xj in &self.final_energy_scores {
                sum_diff += (xi - xj).abs();
            }
        }
        sum_diff / (2.0 * n * n * mean)
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ConsistencyMetrics {
    /// Time until all nodes had same state (None if never converged)
    pub convergence_time: Option<Duration>,
    /// Number of state reconciliation rounds needed
    pub reconciliation_rounds: u32,
    /// Maximum state divergence observed (e.g., missing messages)
    pub max_divergence: usize,
    /// Shannon entropy of state distribution at end (0 = all same, higher = more divergent)
    pub final_entropy: f64,
}

impl ConsistencyMetrics {
    pub fn converged(&self) -> bool {
        self.convergence_time.is_some()
    }

    /// Calculate Shannon Entropy of message distribution across nodes
    pub fn calculate_entropy(node_message_counts: &[usize], total_messages: usize) -> f64 {
        if total_messages == 0 || node_message_counts.is_empty() {
            return 0.0;
        }
        
        let n = node_message_counts.len() as f64;
        let mut entropy = 0.0;
        
        for &count in node_message_counts {
            // Probability that a randomly chosen message-node pair is this node
            let p = count as f64 / (total_messages as f64 * n);
            if p > 0.0 {
                entropy -= p * p.log2();
            }
        }
        entropy
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FaultType {
    /// Network partition between two groups
    Partition { group_a: Vec<String>, group_b: Vec<String> },
    /// Node crashes (stops responding)
    NodeCrash { node_ids: Vec<String> },
    /// Message degradation (drop probability)
    Degradation { drop_probability: f32 },
    /// Network heals after partition
    PartitionHeal,
    /// Node recovers from crash
    NodeRecover { node_ids: Vec<String> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FaultEvent {
    pub time: Duration,
    pub fault: FaultType,
}

/// Evaluation scenario configuration
#[derive(Debug, Clone)]
pub struct EvalScenario {
    pub name: String,
    pub node_count: usize,
    pub publisher_count: usize,
    pub message_rate_per_sec: f32,
    pub message_size_bytes: usize,
    pub duration: Duration,
    pub warmup: Duration,
    pub cooldown: Duration,
    pub fault_schedule: Vec<FaultEvent>,
    /// Percentage of nodes starting with low energy
    pub low_energy_percentage: f32,
    /// Sybil:honest ratio (0 = no sybils)
    pub sybil_ratio: f32,
}

impl Default for EvalScenario {
    fn default() -> Self {
        Self {
            name: "baseline".to_string(),
            node_count: 100,
            publisher_count: 10,
            message_rate_per_sec: 10.0,
            message_size_bytes: 2048,
            duration: Duration::from_secs(60),
            warmup: Duration::from_secs(10),
            cooldown: Duration::from_secs(10),
            fault_schedule: vec![],
            low_energy_percentage: 0.0,
            sybil_ratio: 0.0,
        }
    }
}

impl EvalScenario {
    pub fn baseline(node_count: usize) -> Self {
        Self {
            name: "baseline".to_string(),
            node_count,
            publisher_count: node_count / 10,
            ..Default::default()
        }
    }

    /// Percolation threshold test: sweep dead node percentages
    pub fn percolation_sweep() -> Vec<Self> {
        vec![0, 10, 20, 30, 40, 50, 60, 70, 80, 90]
            .into_iter()
            .map(|pct| Self {
                name: format!("percolation_{}pct_dead", pct),
                low_energy_percentage: pct as f32,
                ..Default::default()
            })
            .collect()
    }

    /// Network degradation attack (like Gossipsub report)
    pub fn degradation_attack(drop_probability: f32) -> Self {
        let inject_time = Duration::from_secs(20);
        Self {
            name: format!("degradation_{:.0}pct", drop_probability * 100.0),
            fault_schedule: vec![FaultEvent {
                time: inject_time,
                fault: FaultType::Degradation { drop_probability },
            }],
            ..Default::default()
        }
    }

    /// Network partition scenario
    pub fn partition_test() -> Self {
        Self {
            name: "network_partition".to_string(),
            fault_schedule: vec![
                FaultEvent {
                    time: Duration::from_secs(20),
                    fault: FaultType::Partition {
                        group_a: (0..50).map(|i| format!("node_{}", i)).collect(),
                        group_b: (50..100).map(|i| format!("node_{}", i)).collect(),
                    },
                },
                FaultEvent {
                    time: Duration::from_secs(40),
                    fault: FaultType::PartitionHeal,
                },
            ],
            ..Default::default()
        }
    }

    /// Cold boot attack (all nodes start together under Sybil pressure)
    pub fn cold_boot_attack(sybil_ratio: f32) -> Self {
        Self {
            name: format!("cold_boot_{}x_sybil", sybil_ratio as u32),
            sybil_ratio,
            warmup: Duration::ZERO, // No warmup - attack from start
            ..Default::default()
        }
    }
}

/// Collector for metrics during evaluation
#[derive(Debug, Default)]
pub struct MetricsCollector {
    start_time: Option<Instant>,
    delivery: DeliveryMetrics,
    energy_samples: Vec<(Duration, Vec<f32>)>,
    consistency_samples: Vec<(Duration, usize)>, // (time, divergence count)
    fault_events: Vec<FaultEvent>,
}

impl MetricsCollector {
    pub fn new() -> Self {
        Self {
            start_time: Some(Instant::now()),
            ..Default::default()
        }
    }

    /// Set expected deliveries based on message count and node count
    pub fn set_expected_deliveries(&mut self, node_count: usize) {
        self.delivery.expected_deliveries = 
            self.delivery.messages_published * node_count as u64;
    }

    pub fn record_publish(&mut self, node_count: usize) {
        self.delivery.messages_published += 1;
        self.delivery.expected_deliveries += node_count as u64;
    }

    pub fn record_delivery(&mut self, latency: Duration) {
        self.delivery.messages_delivered += 1;
        self.delivery.latencies_us.push(latency.as_micros() as u64);
    }

    pub fn record_energy_snapshot(&mut self, scores: Vec<f32>) {
        let elapsed = self.start_time.map(|s| s.elapsed()).unwrap_or_default();
        self.energy_samples.push((elapsed, scores));
    }

    pub fn record_consistency(&mut self, divergence_count: usize) {
        let elapsed = self.start_time.map(|s| s.elapsed()).unwrap_or_default();
        self.consistency_samples.push((elapsed, divergence_count));
    }

    pub fn record_fault(&mut self, fault: FaultType) {
        let elapsed = self.start_time.map(|s| s.elapsed()).unwrap_or_default();
        self.fault_events.push(FaultEvent { time: elapsed, fault });
    }

    pub fn finalize(self, scenario: &EvalScenario, mah_consumed: f32) -> EvalRun {
        let final_scores = self.energy_samples.last()
            .map(|(_, s)| s.clone())
            .unwrap_or_default();
        
        let nodes_exhausted = final_scores.iter().filter(|&&s| s < 0.1).count();
        
        let mah_per_delivery = if self.delivery.messages_delivered > 0 {
            mah_consumed / self.delivery.messages_delivered as f32
        } else {
            0.0
        };

        // Find convergence time (first time divergence hit 0)
        let convergence_time = self.consistency_samples.iter()
            .find(|(_, div)| *div == 0)
            .map(|(t, _)| *t);

        // Calculate final entropy
        let final_entropy = if let Some((_, last_div)) = self.consistency_samples.last() {
            if *last_div == 0 { 0.0 } else { (*last_div as f64).ln() }
        } else {
            0.0
        };

        EvalRun {
            scenario: scenario.name.clone(),
            node_count: scenario.node_count,
            duration: self.start_time.map(|s| s.elapsed()).unwrap_or_default(),
            delivery: self.delivery,
            energy: EnergyMetrics {
                total_mah_consumed: mah_consumed,
                mah_per_delivery,
                nodes_exhausted,
                final_energy_scores: final_scores,
            },
            consistency: ConsistencyMetrics {
                convergence_time,
                reconciliation_rounds: self.consistency_samples.len() as u32,
                max_divergence: self.consistency_samples.iter().map(|(_, d)| *d).max().unwrap_or(0),
                final_entropy,
            },
            fault_events: self.fault_events,
        }
    }
}

/// Summary statistics across multiple runs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EvalSummary {
    pub scenario: String,
    pub runs: usize,
    pub delivery_rate_mean: f64,
    pub delivery_rate_std: f64,
    pub p99_latency_mean_us: f64,
    pub convergence_rate: f64, // % of runs that converged
    pub energy_efficiency_mean: f32,
    pub nodes_exhausted_mean: f64,
}

impl EvalSummary {
    pub fn from_runs(runs: &[EvalRun]) -> Option<Self> {
        if runs.is_empty() {
            return None;
        }

        let scenario = runs[0].scenario.clone();
        let n = runs.len() as f64;

        let delivery_rates: Vec<f64> = runs.iter().map(|r| r.delivery.delivery_rate()).collect();
        let delivery_rate_mean = delivery_rates.iter().sum::<f64>() / n;
        let delivery_rate_std = (delivery_rates.iter()
            .map(|r| (r - delivery_rate_mean).powi(2))
            .sum::<f64>() / n)
            .sqrt();

        let p99s: Vec<f64> = runs.iter()
            .filter_map(|r| r.delivery.p99())
            .map(|d| d.as_micros() as f64)
            .collect();
        let p99_latency_mean_us = if p99s.is_empty() { 0.0 } else {
            p99s.iter().sum::<f64>() / p99s.len() as f64
        };

        let convergence_rate = runs.iter()
            .filter(|r| r.consistency.converged())
            .count() as f64 / n;

        let efficiencies: Vec<f32> = runs.iter().map(|r| r.energy.efficiency_ratio()).collect();
        let energy_efficiency_mean = efficiencies.iter().sum::<f32>() / n as f32;

        let exhausted: Vec<f64> = runs.iter().map(|r| r.energy.nodes_exhausted as f64).collect();
        let nodes_exhausted_mean = exhausted.iter().sum::<f64>() / n;

        Some(Self {
            scenario,
            runs: runs.len(),
            delivery_rate_mean,
            delivery_rate_std,
            p99_latency_mean_us,
            convergence_rate,
            energy_efficiency_mean,
            nodes_exhausted_mean,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_percentile_calculation() {
        let mut metrics = DeliveryMetrics::default();
        // 100 samples from 1000us to 100000us
        metrics.latencies_us = (1..=100).map(|i| i * 1000).collect();
        
        // p50 is at index 50, which is 51000 (1-indexed values)
        let p50 = metrics.p50().unwrap().as_micros();
        assert!(p50 >= 49000 && p50 <= 52000, "p50 was {}", p50);
        
        let p90 = metrics.p90().unwrap().as_micros();
        assert!(p90 >= 89000 && p90 <= 92000, "p90 was {}", p90);
        
        let p99 = metrics.p99().unwrap().as_micros();
        assert!(p99 >= 98000 && p99 <= 100000, "p99 was {}", p99);
    }

    #[test]
    fn test_delivery_rate() {
        let mut metrics = DeliveryMetrics::default();
        metrics.expected_deliveries = 100;  // Must set expected deliveries
        metrics.messages_delivered = 95;
        
        let rate = metrics.delivery_rate();
        assert!((rate - 0.95).abs() < 0.001, "rate was {}", rate);
    }

    #[test]
    fn test_energy_gini() {
        let mut metrics = EnergyMetrics::default();
        
        // Perfect equality
        metrics.final_energy_scores = vec![0.5, 0.5, 0.5, 0.5];
        assert!(metrics.energy_gini() < 0.01);
        
        // Maximum inequality (one node has everything)
        metrics.final_energy_scores = vec![0.0, 0.0, 0.0, 1.0];
        assert!(metrics.energy_gini() > 0.5);
    }

    #[test]
    fn test_scenario_configs() {
        let baseline = EvalScenario::baseline(100);
        assert_eq!(baseline.publisher_count, 10);

        let degradation = EvalScenario::degradation_attack(0.5);
        assert!(!degradation.fault_schedule.is_empty());

        let percolation = EvalScenario::percolation_sweep();
        assert_eq!(percolation.len(), 10);
    }
}
