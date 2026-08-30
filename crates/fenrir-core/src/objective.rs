use crate::observation::Observation;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Objective {
    pub primary: ObjectiveExpr,
    pub constraints: Vec<Constraint>,
    pub secondary: Vec<WeightedObjective>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ObjectiveExpr {
    Metric {
        name: String,
        aggregate: Aggregate,
    },
    Composite {
        weights: HashMap<String, f64>,
        objectives: Vec<ObjectiveExpr>,
    },
    Ratio {
        numerator: Box<ObjectiveExpr>,
        denominator: Box<ObjectiveExpr>,
    },
    Transform {
        expr: Box<ObjectiveExpr>,
        transform: Transform,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Aggregate {
    Mean,
    Median,
    P50,
    P90,
    P95,
    P99,
    P999,
    Max,
    Min,
    StdDev,
    Sum,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Transform {
    Identity,
    Log,
    Sqrt,
    Normalize { min: f64, max: f64 },
    Negate,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WeightedObjective {
    pub weight: f64,
    pub objective: ObjectiveExpr,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Constraint {
    pub metric: String,
    pub op: ConstraintOp,
    pub threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ConstraintOp {
    Gt,
    Gte,
    Lt,
    Lte,
    Eq,
}

impl Objective {
    pub fn evaluate(&self, observation: &Observation) -> Score {
        let primary_score = self.primary.evaluate(observation);
        let feasible = self.satisfies_constraints(observation);
        if !feasible || !primary_score.0.is_finite() {
            return Score::infeasible();
        }

        let mut total = primary_score.0;
        for wo in &self.secondary {
            total += wo.weight * wo.objective.evaluate(observation).0;
        }
        if total.is_finite() {
            Score(total)
        } else {
            Score::infeasible()
        }
    }

    pub fn satisfies_constraints(&self, observation: &Observation) -> bool {
        self.constraints.iter().all(|c| c.check(observation))
    }
}

impl ObjectiveExpr {
    fn evaluate(&self, observation: &Observation) -> Score {
        match self {
            ObjectiveExpr::Metric { name, aggregate } => {
                let value = observation.metrics.get_metric(name, *aggregate);
                Score(value.unwrap_or(f64::MIN))
            }
            ObjectiveExpr::Composite {
                weights,
                objectives,
            } => {
                let mut total = 0.0;
                for (index, obj) in objectives.iter().enumerate() {
                    let weight = weights
                        .get(&obj.label())
                        .or_else(|| weights.get(&index.to_string()))
                        .copied()
                        .unwrap_or(1.0);
                    total += weight * obj.evaluate(observation).0;
                }
                Score(total)
            }
            ObjectiveExpr::Ratio {
                numerator,
                denominator,
            } => {
                let num = numerator.evaluate(observation).0;
                let den = denominator.evaluate(observation).0;
                if den == 0.0 {
                    Score(f64::MIN)
                } else {
                    Score(num / den)
                }
            }
            ObjectiveExpr::Transform { expr, transform } => {
                let val = expr.evaluate(observation).0;
                Score(transform.apply(val))
            }
        }
    }

    fn label(&self) -> String {
        match self {
            ObjectiveExpr::Metric { name, .. } => name.clone(),
            ObjectiveExpr::Composite { .. } => "composite".into(),
            ObjectiveExpr::Ratio { .. } => "ratio".into(),
            ObjectiveExpr::Transform { .. } => "transform".into(),
        }
    }
}

impl Transform {
    fn apply(&self, val: f64) -> f64 {
        match self {
            Transform::Identity => val,
            Transform::Log => val.ln(),
            Transform::Sqrt => val.sqrt(),
            Transform::Normalize { min, max } => {
                if *max <= *min {
                    val
                } else {
                    (val - min) / (max - min)
                }
            }
            Transform::Negate => -val,
        }
    }
}

impl Constraint {
    fn check(&self, observation: &Observation) -> bool {
        let value = observation.metrics.get_scalar(&self.metric);
        if value.is_none_or(|value| !value.is_finite()) {
            return false;
        }
        let v = value.unwrap_or_default();
        match self.op {
            ConstraintOp::Gt => v > self.threshold,
            ConstraintOp::Gte => v >= self.threshold,
            ConstraintOp::Lt => v < self.threshold,
            ConstraintOp::Lte => v <= self.threshold,
            ConstraintOp::Eq => (v - self.threshold).abs() < 1e-9,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, PartialOrd, Serialize, Deserialize)]
pub struct Score(pub f64);

impl Score {
    pub fn feasible(&self) -> bool {
        // `f64::MIN` is the explicit infeasible sentinel. We avoid
        // `f64::NEG_INFINITY` because `serde_json` serializes infinity as
        // `null`, which then fails to deserialize on reload.
        self.0 != f64::MIN
    }

    pub fn infeasible() -> Self {
        Score(f64::MIN)
    }

    pub fn value(&self) -> f64 {
        self.0
    }
}

impl Default for Objective {
    fn default() -> Self {
        Self::p99_latency()
    }
}

impl Objective {
    pub fn p99_latency() -> Self {
        Self {
            primary: ObjectiveExpr::Metric {
                name: "p99_latency_ns".into(),
                aggregate: Aggregate::P99,
            },
            constraints: vec![Constraint {
                metric: "throughput_ops_s".into(),
                op: ConstraintOp::Gte,
                threshold: 1_000_000.0,
            }],
            secondary: vec![],
        }
    }

    pub fn cas_retries() -> Self {
        Self {
            primary: ObjectiveExpr::Metric {
                name: "cas_retries_per_op".into(),
                aggregate: Aggregate::Mean,
            },
            constraints: vec![Constraint {
                metric: "throughput_ops_s".into(),
                op: ConstraintOp::Gte,
                threshold: 500_000.0,
            }],
            secondary: vec![],
        }
    }

    pub fn tail_amplification() -> Self {
        Self {
            primary: ObjectiveExpr::Ratio {
                numerator: Box::new(ObjectiveExpr::Metric {
                    name: "p99_latency_ns".into(),
                    aggregate: Aggregate::P99,
                }),
                denominator: Box::new(ObjectiveExpr::Metric {
                    name: "p50_latency_ns".into(),
                    aggregate: Aggregate::P50,
                }),
            },
            constraints: vec![],
            secondary: vec![],
        }
    }

    /// Maximise p99 latency while requiring a minimum throughput.
    pub fn p99_latency_subject_to_throughput(min_throughput: f64) -> Self {
        Self {
            primary: ObjectiveExpr::Metric {
                name: "p99_latency_ns".into(),
                aggregate: Aggregate::P99,
            },
            constraints: vec![Constraint {
                metric: "throughput_ops_s".into(),
                op: ConstraintOp::Gte,
                threshold: min_throughput,
            }],
            secondary: vec![],
        }
    }

    /// Maximise recovery time without accepting a run that reports a safety
    /// violation.  Targets may expose a scalar `safety_violations` metric.
    pub fn recovery_time_subject_to_safety() -> Self {
        Self {
            primary: ObjectiveExpr::Metric {
                name: "recovery_time_ns".into(),
                aggregate: Aggregate::Max,
            },
            constraints: vec![Constraint {
                metric: "safety_violations".into(),
                op: ConstraintOp::Eq,
                threshold: 0.0,
            }],
            secondary: vec![],
        }
    }
}
