use crate::search_space::SearchSpace;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Genome {
    /// Version of the serialized genome document.
    ///
    /// Keeping this in the document makes old campaign plans distinguishable
    /// from newly-authored plans.  Missing values are interpreted as version 1
    /// for the pre-versioning files already checked into the repository.
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    pub topology: TopologyGenome,
    pub traffic: TrafficGenome,
    pub payload: PayloadGenome,
    pub behavior: BehaviorGenome,
    pub faults: Option<FaultGenome>,
}

pub const CURRENT_SCHEMA_VERSION: u32 = 1;

fn default_schema_version() -> u32 {
    CURRENT_SCHEMA_VERSION
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TopologyGenome {
    pub producers: ParameterSpec,
    pub consumers: ParameterSpec,
    pub affinity: AffinitySpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TrafficGenome {
    pub arrival_process: ArrivalProcess,
    pub rate: ParameterSpec,
    pub burstiness: ParameterSpec,
    pub correlation: ParameterSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PayloadGenome {
    pub size_distribution: SizeDistribution,
    pub size: ParameterSpec,
    pub key_distribution: KeyDistribution,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct BehaviorGenome {
    pub read_write_ratio: ParameterSpec,
    pub lifetime_ns: ParameterSpec,
    pub think_time_ns: ParameterSpec,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum ParameterSpec {
    Fixed {
        value: serde_json::Value,
    },
    Range {
        min: f64,
        max: f64,
        log: bool,
    },
    Integer {
        min: i64,
        max: i64,
    },
    Choice {
        values: Vec<serde_json::Value>,
    },
    Distribution {
        dist: DistributionSpec,
    },
    LogUniform {
        min: f64,
        max: f64,
    },
    /// A parameter whose domain is selected by another sampled parameter.
    ///
    /// The coordinate is still represented in the optimizer's fixed vector;
    /// callers that do not satisfy the condition omit the value from the
    /// adapter document.  This keeps existing optimizers compatible while
    /// making conditional dimensions explicit in the genome schema.
    Conditional {
        depends_on: String,
        equals: serde_json::Value,
        spec: Box<ParameterSpec>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type")]
pub enum DistributionSpec {
    Uniform { min: f64, max: f64 },
    LogUniform { min: f64, max: f64 },
    Normal { mean: f64, std: f64 },
    Exponential { lambda: f64 },
    Beta { alpha: f64, beta: f64 },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ArrivalProcess {
    Poisson,
    Burst,
    Pareto,
    Constant,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SizeDistribution {
    Fixed,
    Uniform,
    LogNormal,
    Pareto,
    Bimodal,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum KeyDistribution {
    Uniform,
    Zipfian,
    HotSpot,
    Sequential,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AffinitySpec {
    None,
    Socket,
    Core,
    SmtSibling,
    Custom(String),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FaultGenome {
    pub enabled: bool,
    pub scenario_template: String,
    pub parameters: HashMap<String, ParameterSpec>,
}

impl Genome {
    pub fn validate(&self) -> Result<(), GenomeError> {
        if self.schema_version == 0 || self.schema_version > CURRENT_SCHEMA_VERSION {
            return Err(GenomeError::UnsupportedSchemaVersion(self.schema_version));
        }
        self.topology.validate()?;
        self.traffic.validate()?;
        self.payload.validate()?;
        self.behavior.validate()?;
        if let Some(faults) = &self.faults {
            faults.validate()?;
        }
        Ok(())
    }

    pub fn search_space(&self) -> SearchSpace {
        SearchSpace::from_genome(self)
    }

    pub fn default_norn() -> Self {
        Self {
            schema_version: CURRENT_SCHEMA_VERSION,
            topology: TopologyGenome {
                producers: ParameterSpec::Range {
                    min: 1.0,
                    max: 32.0,
                    log: false,
                },
                consumers: ParameterSpec::Range {
                    min: 1.0,
                    max: 32.0,
                    log: false,
                },
                affinity: AffinitySpec::Core,
            },
            traffic: TrafficGenome {
                arrival_process: ArrivalProcess::Poisson,
                rate: ParameterSpec::LogUniform {
                    min: 10_000.0,
                    max: 5_000_000.0,
                },
                burstiness: ParameterSpec::Range {
                    min: 0.0,
                    max: 1.0,
                    log: false,
                },
                correlation: ParameterSpec::Range {
                    min: 0.0,
                    max: 1.0,
                    log: false,
                },
            },
            payload: PayloadGenome {
                size_distribution: SizeDistribution::Fixed,
                size: ParameterSpec::Choice {
                    values: vec![
                        serde_json::json!(64),
                        serde_json::json!(128),
                        serde_json::json!(192),
                        serde_json::json!(256),
                        serde_json::json!(512),
                        serde_json::json!(1024),
                    ],
                },
                key_distribution: KeyDistribution::Uniform,
            },
            behavior: BehaviorGenome {
                read_write_ratio: ParameterSpec::Range {
                    min: 0.0,
                    max: 1.0,
                    log: false,
                },
                lifetime_ns: ParameterSpec::LogUniform {
                    min: 1_000.0,
                    max: 10_000_000.0,
                },
                think_time_ns: ParameterSpec::LogUniform {
                    min: 1.0,
                    max: 100_000.0,
                },
            },
            faults: None,
        }
    }
}

impl TopologyGenome {
    fn validate(&self) -> Result<(), GenomeError> {
        validate_positive(&self.producers, "topology.producers")?;
        validate_positive(&self.consumers, "topology.consumers")?;
        Ok(())
    }
}

impl TrafficGenome {
    fn validate(&self) -> Result<(), GenomeError> {
        validate_positive(&self.rate, "traffic.rate")?;
        validate_bounded(&self.burstiness, 0.0, 1.0, "traffic.burstiness")?;
        validate_bounded(&self.correlation, 0.0, 1.0, "traffic.correlation")?;
        Ok(())
    }
}

impl PayloadGenome {
    fn validate(&self) -> Result<(), GenomeError> {
        validate_positive(&self.size, "payload.size")?;
        Ok(())
    }
}

impl BehaviorGenome {
    fn validate(&self) -> Result<(), GenomeError> {
        validate_bounded(
            &self.read_write_ratio,
            0.0,
            1.0,
            "behavior.read_write_ratio",
        )?;
        validate_positive(&self.lifetime_ns, "behavior.lifetime_ns")?;
        validate_nonnegative(&self.think_time_ns, "behavior.think_time_ns")?;
        Ok(())
    }
}

impl FaultGenome {
    fn validate(&self) -> Result<(), GenomeError> {
        if self.enabled && self.scenario_template.is_empty() {
            return Err(GenomeError::InvalidFaultConfig(
                "scenario_template required when faults.enabled=true".into(),
            ));
        }
        Ok(())
    }
}

fn validate_positive(spec: &ParameterSpec, path: &str) -> Result<(), GenomeError> {
    match spec {
        ParameterSpec::Fixed { value } => {
            if let Some(n) = value.as_f64()
                && n <= 0.0
            {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: "must be positive".into(),
                });
            }
        }
        ParameterSpec::Range { min, .. }
        | ParameterSpec::LogUniform { min, .. }
        | ParameterSpec::Distribution {
            dist: DistributionSpec::LogUniform { min, .. },
        } => {
            if *min <= 0.0 {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: "min must be positive".into(),
                });
            }
        }
        ParameterSpec::Integer { min, .. } => {
            if *min <= 0 {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: "min must be positive".into(),
                });
            }
        }
        ParameterSpec::Choice { values } => {
            for v in values {
                if let Some(n) = v.as_f64()
                    && n <= 0.0
                {
                    return Err(GenomeError::InvalidParameter {
                        path: path.into(),
                        reason: "all choice values must be positive".into(),
                    });
                }
            }
        }
        ParameterSpec::Conditional { spec, .. } => validate_positive(spec, path)?,
        _ => {}
    }
    Ok(())
}

fn validate_nonnegative(spec: &ParameterSpec, path: &str) -> Result<(), GenomeError> {
    match spec {
        ParameterSpec::Fixed { value } => {
            if let Some(n) = value.as_f64()
                && n < 0.0
            {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: "must be non-negative".into(),
                });
            }
        }
        ParameterSpec::Range { min, .. }
        | ParameterSpec::LogUniform { min, .. }
        | ParameterSpec::Distribution {
            dist: DistributionSpec::LogUniform { min, .. },
        } => {
            if *min < 0.0 {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: "min must be non-negative".into(),
                });
            }
        }
        ParameterSpec::Integer { min, .. } => {
            if *min < 0 {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: "min must be non-negative".into(),
                });
            }
        }
        ParameterSpec::Choice { values } => {
            for v in values {
                if let Some(n) = v.as_f64()
                    && n < 0.0
                {
                    return Err(GenomeError::InvalidParameter {
                        path: path.into(),
                        reason: "all choice values must be non-negative".into(),
                    });
                }
            }
        }
        ParameterSpec::Conditional { spec, .. } => validate_nonnegative(spec, path)?,
        _ => {}
    }
    Ok(())
}

fn validate_bounded(
    spec: &ParameterSpec,
    min_allowed: f64,
    max_allowed: f64,
    path: &str,
) -> Result<(), GenomeError> {
    match spec {
        ParameterSpec::Fixed { value } => {
            if let Some(n) = value.as_f64()
                && (n < min_allowed || n > max_allowed)
            {
                return Err(GenomeError::InvalidParameter {
                    path: path.into(),
                    reason: format!("must be in [{}, {}]", min_allowed, max_allowed),
                });
            }
        }
        ParameterSpec::Range { min, max, .. } if *min < min_allowed || *max > max_allowed => {
            return Err(GenomeError::InvalidParameter {
                path: path.into(),
                reason: format!("range must be within [{}, {}]", min_allowed, max_allowed),
            });
        }
        ParameterSpec::Integer { min, max }
            if (*min as f64) < min_allowed || (*max as f64) > max_allowed =>
        {
            return Err(GenomeError::InvalidParameter {
                path: path.into(),
                reason: format!("range must be within [{}, {}]", min_allowed, max_allowed),
            });
        }
        ParameterSpec::Conditional { spec, .. } => {
            validate_bounded(spec, min_allowed, max_allowed, path)?
        }
        _ => {}
    }
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum GenomeError {
    #[error("invalid parameter {path}: {reason}")]
    InvalidParameter { path: String, reason: String },
    #[error("invalid fault config: {0}")]
    InvalidFaultConfig(String),
    #[error("unsupported genome schema version: {0}")]
    UnsupportedSchemaVersion(u32),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SampledGenome {
    pub values: HashMap<String, f64>,
    pub target_config: serde_json::Value,
}

impl SampledGenome {
    pub fn get(&self, path: &str) -> Option<f64> {
        self.values.get(path).copied()
    }

    pub fn get_u64(&self, path: &str) -> Option<u64> {
        self.values.get(path).map(|v| *v as u64)
    }

    pub fn get_usize(&self, path: &str) -> Option<usize> {
        self.values.get(path).map(|v| *v as usize)
    }
}

impl ParameterSpec {
    /// Decode a sampled *coordinate* into the actual parameter value.
    ///
    /// For `Choice`, the search coordinate is an index into `values`
    /// (CMA-ES/random can produce fractional or out-of-range indices), so we
    /// round to the nearest choice and clamp. Every other spec passes the
    /// coordinate through unchanged. This is the missing link between the
    /// optimizer's coordinate space and the value the target adapter expects.
    pub fn decode(&self, coord: f64) -> f64 {
        match self {
            ParameterSpec::Choice { values } => {
                if values.is_empty() {
                    return coord;
                }
                let n = values.len() as i64;
                let mut idx = coord.round() as i64;
                if idx < 0 {
                    idx = 0;
                }
                if idx >= n {
                    idx = n - 1;
                }
                values[idx as usize].as_f64().unwrap_or(coord)
            }
            ParameterSpec::Integer { min, max } => coord.round().clamp(*min as f64, *max as f64),
            ParameterSpec::Conditional { spec, .. } => spec.decode(coord),
            _ => coord,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn payload_size_spec() -> ParameterSpec {
        ParameterSpec::Choice {
            values: vec![
                serde_json::json!(64),
                serde_json::json!(128),
                serde_json::json!(192),
                serde_json::json!(256),
                serde_json::json!(512),
                serde_json::json!(1024),
            ],
        }
    }

    #[test]
    fn choice_decode_maps_index_to_value() {
        let spec = payload_size_spec();
        assert_eq!(spec.decode(0.0), 64.0);
        assert_eq!(spec.decode(1.0), 128.0);
        assert_eq!(spec.decode(2.0), 192.0);
        assert_eq!(spec.decode(3.0), 256.0);
        assert_eq!(spec.decode(4.0), 512.0);
        assert_eq!(spec.decode(5.0), 1024.0);
    }

    #[test]
    fn choice_decode_rounds_and_clamps() {
        let spec = payload_size_spec();
        assert_eq!(spec.decode(2.4), 192.0, "rounds to nearest index");
        assert_eq!(spec.decode(4.4), 512.0, "rounds to nearest index");
        assert_eq!(spec.decode(-1.0), 64.0, "clamps below range");
        assert_eq!(spec.decode(99.0), 1024.0, "clamps above range");
    }

    #[test]
    fn non_choice_passes_through() {
        let spec = ParameterSpec::Range {
            min: 0.0,
            max: 1.0,
            log: false,
        };
        assert_eq!(spec.decode(0.5), 0.5);
    }

    #[test]
    fn missing_schema_version_uses_current_version() {
        let value = serde_json::to_value(Genome::default_norn()).unwrap();
        let mut object = value.as_object().cloned().unwrap();
        object.remove("schema_version");
        let parsed: Genome = serde_json::from_value(object.into()).unwrap();
        assert_eq!(parsed.schema_version, CURRENT_SCHEMA_VERSION);
        assert!(parsed.validate().is_ok());
    }

    #[test]
    fn future_schema_version_is_rejected() {
        let mut value = serde_json::to_value(Genome::default_norn()).unwrap();
        value["schema_version"] = serde_json::json!(CURRENT_SCHEMA_VERSION + 1);
        let parsed: Genome = serde_json::from_value(value).unwrap();
        assert!(matches!(
            parsed.validate(),
            Err(GenomeError::UnsupportedSchemaVersion(_))
        ));
    }

    #[test]
    fn conditional_spec_preserves_inner_domain_and_decoding() {
        let spec = ParameterSpec::Conditional {
            depends_on: "mode".into(),
            equals: serde_json::json!("fast"),
            spec: Box::new(payload_size_spec()),
        };
        assert_eq!(spec.decode(2.0), 192.0);
        let genome = Genome::default_norn();
        let space = genome.search_space();
        assert_eq!(space.dimension(), 9);
    }

    #[test]
    fn integer_spec_rounds_and_clamps() {
        let spec = ParameterSpec::Integer { min: 2, max: 6 };
        assert_eq!(spec.decode(3.6), 4.0);
        assert_eq!(spec.decode(-1.0), 2.0);
        assert_eq!(spec.decode(99.0), 6.0);
    }
}
