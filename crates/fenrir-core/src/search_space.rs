use crate::genome::{Genome, ParameterSpec, SampledGenome};
use nalgebra::DVector;
use rand::Rng;
use std::collections::HashMap;

pub struct SearchSpace {
    pub param_names: Vec<String>,
    pub bounds: Vec<(f64, f64)>,
    pub discrete_indices: Vec<usize>,
    pub categorical_indices: Vec<usize>,
    pub param_types: Vec<ParamType>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::genome::{BehaviorGenome, Genome, ParameterSpec, TopologyGenome, TrafficGenome};

    #[test]
    fn integer_dimensions_are_discrete() {
        let genome = Genome {
            schema_version: crate::genome::CURRENT_SCHEMA_VERSION,
            topology: TopologyGenome {
                producers: ParameterSpec::Integer { min: 1, max: 4 },
                consumers: ParameterSpec::Integer { min: 1, max: 4 },
                affinity: crate::genome::AffinitySpec::Core,
            },
            traffic: TrafficGenome {
                arrival_process: crate::genome::ArrivalProcess::Constant,
                rate: ParameterSpec::Range { min: 1.0, max: 2.0, log: false },
                burstiness: ParameterSpec::Range { min: 0.0, max: 1.0, log: false },
                correlation: ParameterSpec::Range { min: 0.0, max: 1.0, log: false },
            },
            payload: crate::genome::PayloadGenome {
                size_distribution: crate::genome::SizeDistribution::Fixed,
                size: ParameterSpec::Choice { values: vec![serde_json::json!(1)] },
                key_distribution: crate::genome::KeyDistribution::Uniform,
            },
            behavior: BehaviorGenome {
                read_write_ratio: ParameterSpec::Range { min: 0.0, max: 1.0, log: false },
                lifetime_ns: ParameterSpec::Range { min: 1.0, max: 2.0, log: false },
                think_time_ns: ParameterSpec::Range { min: 0.0, max: 1.0, log: false },
            },
            faults: None,
        };
        let space = genome.search_space();
        assert_eq!(space.discrete_indices, vec![0, 1]);
        assert_eq!(space.param_types[0], ParamType::Discrete);
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ParamType {
    Continuous,
    Discrete,
    Categorical,
    LogContinuous,
}

impl SearchSpace {
    pub fn from_genome(genome: &Genome) -> Self {
        let mut param_names = Vec::new();
        let mut bounds = Vec::new();
        let mut discrete_indices = Vec::new();
        let mut categorical_indices = Vec::new();
        let mut param_types = Vec::new();

        Self::extract_spec(
            &genome.topology,
            "topology",
            &mut param_names,
            &mut bounds,
            &mut discrete_indices,
            &mut categorical_indices,
            &mut param_types,
        );
        Self::extract_spec(
            &genome.traffic,
            "traffic",
            &mut param_names,
            &mut bounds,
            &mut discrete_indices,
            &mut categorical_indices,
            &mut param_types,
        );
        Self::extract_spec(
            &genome.payload,
            "payload",
            &mut param_names,
            &mut bounds,
            &mut discrete_indices,
            &mut categorical_indices,
            &mut param_types,
        );
        Self::extract_spec(
            &genome.behavior,
            "behavior",
            &mut param_names,
            &mut bounds,
            &mut discrete_indices,
            &mut categorical_indices,
            &mut param_types,
        );

        if let Some(faults) = &genome.faults {
            Self::extract_fault_spec(
                faults,
                &mut param_names,
                &mut bounds,
                &mut discrete_indices,
                &mut categorical_indices,
                &mut param_types,
            );
        }

        Self {
            param_names,
            bounds,
            discrete_indices,
            categorical_indices,
            param_types,
        }
    }

    fn extract_spec<T>(
        obj: &T,
        prefix: &str,
        names: &mut Vec<String>,
        bounds: &mut Vec<(f64, f64)>,
        discrete: &mut Vec<usize>,
        categorical: &mut Vec<usize>,
        types: &mut Vec<ParamType>,
    ) where
        T: serde::Serialize,
    {
        let value = serde_json::to_value(obj).unwrap();
        Self::extract_from_json(&value, prefix, names, bounds, discrete, categorical, types);
    }

    fn extract_fault_spec(
        faults: &crate::genome::FaultGenome,
        names: &mut Vec<String>,
        bounds: &mut Vec<(f64, f64)>,
        discrete: &mut Vec<usize>,
        categorical: &mut Vec<usize>,
        types: &mut Vec<ParamType>,
    ) {
        for (key, spec) in &faults.parameters {
            let idx = names.len();
            names.push(format!("faults.{}", key));
            Self::extract_param_spec(spec, idx, bounds, types, discrete, categorical);
        }
    }

    fn extract_from_json(
        value: &serde_json::Value,
        prefix: &str,
        names: &mut Vec<String>,
        bounds: &mut Vec<(f64, f64)>,
        discrete: &mut Vec<usize>,
        categorical: &mut Vec<usize>,
        types: &mut Vec<ParamType>,
    ) {
        if let Some(obj) = value.as_object() {
            for (key, val) in obj {
                let path = format!("{}.{}", prefix, key);
                if val.is_object() {
                    // Check if it's a ParameterSpec enum with "type" field
                    if let Some(type_val) = val.get("type") {
                        if let Some(type_str) = type_val.as_str() {
                            let idx = names.len();
                            names.push(path);
                            Self::extract_param_spec_from_type(
                                type_str,
                                val,
                                idx,
                                bounds,
                                types,
                                discrete,
                                categorical,
                            );
                        } else {
                            Self::extract_from_json(
                                val,
                                &path,
                                names,
                                bounds,
                                discrete,
                                categorical,
                                types,
                            );
                        }
                    } else {
                        Self::extract_from_json(
                            val,
                            &path,
                            names,
                            bounds,
                            discrete,
                            categorical,
                            types,
                        );
                    }
                }
            }
        }
    }

    fn extract_param_spec_from_type(
        type_str: &str,
        spec_obj: &serde_json::Value,
        idx: usize,
        bounds: &mut Vec<(f64, f64)>,
        types: &mut Vec<ParamType>,
        discrete: &mut Vec<usize>,
        categorical: &mut Vec<usize>,
    ) {
        match type_str {
            "Fixed" => {
                if let Some(f) = spec_obj.get("value").and_then(|v| v.as_f64()) {
                    bounds.push((f, f));
                    types.push(ParamType::Continuous);
                }
            }
            "Range" => {
                let min = spec_obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let max = spec_obj.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0);
                let log = spec_obj
                    .get("log")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                bounds.push((min, max));
                types.push(if log {
                    ParamType::LogContinuous
                } else {
                    ParamType::Continuous
                });
            }
            "Integer" => {
                let min = spec_obj.get("min").and_then(|v| v.as_i64()).unwrap_or(0);
                let max = spec_obj.get("max").and_then(|v| v.as_i64()).unwrap_or(min);
                bounds.push((min as f64, max as f64));
                types.push(ParamType::Discrete);
                discrete.push(idx);
            }
            "Choice" => {
                if let Some(values) = spec_obj.get("values").and_then(|v| v.as_array()) {
                    bounds.push((0.0, values.len() as f64 - 1.0));
                    types.push(ParamType::Categorical);
                    categorical.push(idx);
                }
            }
            "LogUniform" => {
                let min = spec_obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
                let max = spec_obj.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0);
                bounds.push((min, max));
                types.push(ParamType::LogContinuous);
            }
            "Distribution" => {
                if let Some(dist_obj) = spec_obj.get("dist") {
                    Self::extract_distribution(dist_obj, idx, bounds, types);
                }
            }
            "Conditional" => {
                if let Some(inner) = spec_obj.get("spec") {
                    let inner_type = inner
                        .get("type")
                        .and_then(|v| v.as_str())
                        .unwrap_or("Range");
                    Self::extract_param_spec_from_type(
                        inner_type,
                        inner,
                        idx,
                        bounds,
                        types,
                        discrete,
                        categorical,
                    );
                } else {
                    bounds.push((0.0, 1.0));
                    types.push(ParamType::Continuous);
                }
            }
            _ => {
                bounds.push((0.0, 1.0));
                types.push(ParamType::Continuous);
            }
        }
    }

    fn extract_distribution(
        dist: &serde_json::Value,
        _idx: usize,
        bounds: &mut Vec<(f64, f64)>,
        types: &mut Vec<ParamType>,
    ) {
        if let Some(obj) = dist.as_object() {
            for (variant, spec_obj) in obj {
                match variant.as_str() {
                    "Uniform" => {
                        let min = spec_obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let max = spec_obj.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        bounds.push((min, max));
                        types.push(ParamType::Continuous);
                    }
                    "LogUniform" => {
                        let min = spec_obj.get("min").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let max = spec_obj.get("max").and_then(|v| v.as_f64()).unwrap_or(1.0);
                        bounds.push((min, max));
                        types.push(ParamType::LogContinuous);
                    }
                    _ => {
                        bounds.push((0.0, 1.0));
                        types.push(ParamType::Continuous);
                    }
                }
            }
        }
    }

    fn extract_param_spec(
        spec: &ParameterSpec,
        idx: usize,
        bounds: &mut Vec<(f64, f64)>,
        types: &mut Vec<ParamType>,
        discrete: &mut Vec<usize>,
        categorical: &mut Vec<usize>,
    ) {
        match spec {
            ParameterSpec::Fixed { value } => {
                if let Some(f) = value.as_f64() {
                    bounds.push((f, f));
                    types.push(ParamType::Continuous);
                }
            }
            ParameterSpec::Range { min, max, log } => {
                bounds.push((*min, *max));
                types.push(if *log {
                    ParamType::LogContinuous
                } else {
                    ParamType::Continuous
                });
            }
            ParameterSpec::Integer { min, max } => {
                bounds.push((*min as f64, *max as f64));
                types.push(ParamType::Discrete);
                discrete.push(idx);
            }
            ParameterSpec::Choice { values } => {
                bounds.push((0.0, values.len() as f64 - 1.0));
                types.push(ParamType::Categorical);
                categorical.push(idx);
            }
            ParameterSpec::LogUniform { min, max } => {
                bounds.push((*min, *max));
                types.push(ParamType::LogContinuous);
            }
            ParameterSpec::Distribution { dist } => match dist {
                crate::genome::DistributionSpec::Uniform { min, max } => {
                    bounds.push((*min, *max));
                    types.push(ParamType::Continuous);
                }
                crate::genome::DistributionSpec::LogUniform { min, max } => {
                    bounds.push((*min, *max));
                    types.push(ParamType::LogContinuous);
                }
                _ => {
                    bounds.push((0.0, 1.0));
                    types.push(ParamType::Continuous);
                }
            },
            ParameterSpec::Conditional { spec, .. } => {
                Self::extract_param_spec(spec, idx, bounds, types, discrete, categorical)
            }
        }
    }

    pub fn dimension(&self) -> usize {
        self.param_names.len()
    }

    pub fn to_vector(&self, genome: &SampledGenome) -> DVector<f64> {
        let mut vec = DVector::zeros(self.dimension());
        for (i, name) in self.param_names.iter().enumerate() {
            if let Some(val) = genome.values.get(name) {
                vec[i] = *val;
            }
        }
        vec
    }

    pub fn from_vector(&self, vec: &DVector<f64>) -> SampledGenome {
        let mut values = HashMap::new();
        for (i, name) in self.param_names.iter().enumerate() {
            values.insert(name.clone(), vec[i]);
        }
        SampledGenome {
            values,
            target_config: serde_json::Value::Null,
        }
    }

    pub fn random_sample(&self, rng: &mut impl Rng) -> SampledGenome {
        let mut values = HashMap::new();
        for (i, name) in self.param_names.iter().enumerate() {
            let (min, max) = self.bounds[i];
            let val = match self.param_types[i] {
                ParamType::Continuous => rng.gen_range(min..=max),
                ParamType::LogContinuous => {
                    let log_min = min.ln();
                    let log_max = max.ln();
                    (rng.gen_range(log_min..=log_max)).exp()
                }
                ParamType::Discrete => rng.gen_range(min as i64..=max as i64) as f64,
                ParamType::Categorical => rng.gen_range(0..=(max as usize)) as f64,
            };
            values.insert(name.clone(), val);
        }
        SampledGenome {
            values,
            target_config: serde_json::Value::Null,
        }
    }

    pub fn clip(&self, vec: &mut DVector<f64>) {
        for i in 0..self.dimension() {
            let (min, max) = self.bounds[i];
            vec[i] = vec[i].clamp(min, max);
        }
    }
}
