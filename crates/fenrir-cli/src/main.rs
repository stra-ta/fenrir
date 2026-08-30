use clap::{Parser, Subcommand};
use anyhow::Context;
use fenrir_core::{Genome, Objective};
use fenrir_runner::evaluate_genome;
use fenrir_search::{
    BayesianOptimizer, CmaEsOptimizer, Evaluation, MapElitesOptimizer, Optimizer,
    OptimizerCheckpoint, RandomSearch,
};
use fenrir_store::CampaignStore;
use fenrir_store::CampaignManifest;
use fenrir_targets::get_target;
use figment::providers::{Format, Toml};
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::Semaphore;
use tracing::{info, warn};

#[derive(Parser)]
#[command(
    name = "fenrir",
    version,
    about = "Adversarial workload synthesis engine"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Hunt for pathological workloads
    Hunt {
        /// Target system (norn, weir, orda)
        #[arg(long, default_value = "norn")]
        target: String,

        /// Genome config file (TOML)
        #[arg(long, short)]
        genome: Option<PathBuf>,

        /// Objective config file (TOML)
        #[arg(long, short = 'j')]
        objective: Option<PathBuf>,

        /// Optimizer: random, cmaes, bayesian, map-elites
        #[arg(long, default_value = "random")]
        optimizer: String,

        /// Total evaluation budget
        #[arg(long, default_value = "100")]
        budget: usize,

        /// Parallel evaluations
        #[arg(long, default_value = "1")]
        parallel: usize,

        /// Duration per evaluation (seconds)
        #[arg(long, default_value = "10")]
        duration: u64,

        /// Campaign seed (u64) for reproducible hunts
        #[arg(long, default_value_t = 42)]
        seed: u64,

        /// Output directory
        #[arg(long, short, default_value = "experiments/fenrir_hunt")]
        output: PathBuf,

        /// Adapter binary path (auto-detected if not specified)
        #[arg(long)]
        adapter: Option<PathBuf>,

        /// Execution backend: local (subprocess adapter) or kiln (Kiln runner)
        #[arg(long, default_value = "local")]
        backend: String,

        /// Resume an interrupted campaign from `optimizer.json` in the output
        /// directory, preserving optimizer and RNG state.
        #[arg(long)]
        resume: bool,
    },

    /// Analyze campaign results
    Analyze {
        /// Campaign directory
        campaign: PathBuf,

        /// Number of top results to show
        #[arg(long, default_value = "10")]
        top: usize,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let cli = Cli::parse();

    match cli.command {
        Commands::Hunt {
            target,
            genome,
            objective,
            optimizer,
            budget,
            parallel,
            duration,
            seed,
            output,
            adapter,
            backend,
            resume,
        } => {
            hunt(
                target, genome, objective, optimizer, budget, parallel, duration, seed, output,
                adapter, backend, resume,
            )
            .await?;
        }
        Commands::Analyze { campaign, top } => {
            analyze(campaign, top)?;
        }
    }

    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn hunt(
    target: String,
    genome_path: Option<PathBuf>,
    objective_path: Option<PathBuf>,
    optimizer_name: String,
    budget: usize,
    parallel: usize,
    duration: u64,
    seed: u64,
    output_dir: PathBuf,
    adapter_path: Option<PathBuf>,
    backend: String,
    resume: bool,
) -> anyhow::Result<()> {
    if budget == 0 {
        anyhow::bail!("budget must be greater than zero");
    }
    if parallel == 0 {
        anyhow::bail!("parallel must be greater than zero");
    }
    if backend != "local" && backend != "kiln" {
        anyhow::bail!("unknown backend '{}'; expected local or kiln", backend);
    }
    info!(
        "Starting Fenrir hunt: target={}, budget={}, parallel={}, backend={}",
        target, budget, parallel, backend
    );

    // Resolve target implementation (owns genome + adapter mapping).
    let target = get_target(&target).ok_or_else(|| {
        anyhow::anyhow!("Unknown target: {}. Known: norn, weir, loki, orda", target)
    })?;

    // An explicit adapter must be the path used by the runner, not only an
    // existence check.  Keep the target's mapping while overriding its binary.
    let resolved_adapter = adapter_path.clone().unwrap_or_else(|| target.adapter_path());
    let target: Arc<dyn fenrir_targets::Target> = match adapter_path {
        Some(path) => Arc::from(fenrir_targets::with_adapter_path(target, path)),
        None => Arc::from(target),
    };

    // Load genome
    let genome = if let Some(path) = genome_path {
        figment::Figment::from(Toml::file(&path)).extract::<Genome>()?
    } else {
        target.default_genome()
    };
    genome.validate()?;

    // Load objective
    let objective = if let Some(path) = objective_path {
        figment::Figment::from(Toml::file(&path)).extract::<Objective>()?
    } else {
        Objective::default()
    };

    // Determine adapter path (used for existence check in local backend).
    if backend == "local" && !resolved_adapter.exists() {
        anyhow::bail!(
            "Adapter not found at {:?}. Build it first.",
            resolved_adapter
        );
    }
    // Create output directory
    std::fs::create_dir_all(&output_dir)?;

    // Initialize store
    let mut store = CampaignStore::new(&output_dir)?;
    let repo_root = std::env::var_os("CARGO_MANIFEST_DIR")
        .map(PathBuf::from)
        .map(|path| path.join("..").join(".."))
        .unwrap_or_else(|| PathBuf::from("."));
    let manifest_path = output_dir.join("results").join("manifest.json");
    let root_manifest_path = output_dir.join("manifest.json");
    let mut manifest = if resume && manifest_path.exists() {
        let bytes = std::fs::read(&manifest_path).with_context(|| {
            format!("failed to read campaign manifest {}", manifest_path.display())
        })?;
        serde_json::from_slice(&bytes).with_context(|| {
            format!("invalid campaign manifest {}", manifest_path.display())
        })?
    } else {
        CampaignManifest::capture(
            target.name().to_string(),
            optimizer_name.clone(),
            budget,
            duration,
            parallel,
            seed,
            backend.clone(),
            &repo_root,
        )
    };
    if manifest.target != target.name()
        || manifest.optimizer != optimizer_name
        || manifest.budget != budget
        || manifest.seed != seed
    {
        anyhow::bail!("campaign manifest does not match the requested resume plan");
    }
    manifest.write_atomic(&manifest_path)?;
    manifest.write_atomic(&root_manifest_path)?;

    // Create search space and optimizer
    let search_space = genome.search_space();
    info!("Search space dimension: {}", search_space.dimension());

    let mut optimizer: Box<dyn Optimizer> = match optimizer_name.as_str() {
        "random" => Box::new(RandomSearch::new(genome.search_space(), budget, seed)),
        "cmaes" => Box::new(CmaEsOptimizer::new(genome.search_space(), budget, seed)),
        "bayesian" => Box::new(BayesianOptimizer::new(genome.search_space(), budget, seed)),
        "map-elites" => Box::new(MapElitesOptimizer::new(genome.search_space(), budget, seed)),
        _ => anyhow::bail!("Unknown optimizer: {}", optimizer_name),
    };

    let checkpoint_path = output_dir.join("optimizer.json");
    if resume && checkpoint_path.exists() {
        let bytes = std::fs::read(&checkpoint_path).with_context(|| {
            format!("failed to read optimizer checkpoint {}", checkpoint_path.display())
        })?;
        let checkpoint: OptimizerCheckpoint = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid optimizer checkpoint {}", checkpoint_path.display()))?;
        optimizer.restore(&checkpoint)?;
        info!("Resumed optimizer from {}", checkpoint_path.display());
    }

    // Semaphore for parallel execution
    let semaphore = Arc::new(Semaphore::new(parallel));
    let mut iteration = store.load_all()?.len() as u64;

    // Run optimization loop
    while !optimizer.is_done() {
        let mut handles = Vec::new();

        for _ in 0..parallel {
            if optimizer.is_done() {
                break;
            }

            let genomes = optimizer.suggest(&[]);
            if genomes.is_empty() {
                break;
            }

            let genome = genomes.into_iter().next().unwrap();
            let target = target.clone();
            let backend = backend.clone();
            let obj = objective.clone();
            let semaphore = semaphore.clone();
            let iter = iteration;

            iteration += 1;

            let eval_seed = seed.wrapping_add(iter.wrapping_mul(0x9E3779B97F4A7C15u64));

            let handle = tokio::spawn(async move {
                let permit = semaphore.acquire().await;
                let _permit = match permit {
                    Ok(p) => p,
                    Err(_) => return (None, Some(anyhow::anyhow!("Semaphore closed"))),
                };
                let observation = match backend.as_str() {
                    "kiln" => {
                        fenrir_runner::kiln::run_with_kiln(&*target, &genome, duration, eval_seed)
                            .await
                    }
                    _ => evaluate_genome(&*target, &genome, duration, eval_seed).await,
                };
                match observation {
                    Ok(observation) => {
                        let score = obj.evaluate(&observation);
                        let eval = Evaluation {
                            iteration: iter,
                            genome,
                            score,
                            observation,
                        };
                        (Some(eval), None)
                    }
                    Err(e) => (None, Some(e)),
                }
            });

            handles.push(handle);
        }

        // Wait for all tasks
        let mut evaluations = Vec::new();
        for handle in handles {
            match handle.await {
                Ok((Some(eval), None)) => {
                    info!(
                        "Iteration {}: score={:.4}, feasible={}",
                        eval.iteration,
                        eval.score.value(),
                        eval.score.feasible()
                    );
                    store.append(&eval)?;
                    evaluations.push(eval);
                }
                Ok((None, Some(e))) => {
                    warn!("Evaluation failed: {}", e);
                }
                Ok((None, None)) => {
                    warn!("Task returned no evaluation and no error");
                }
                Ok((Some(_), Some(_))) => {
                    warn!("Task returned both evaluation and error - unexpected");
                }
                Err(e) => {
                    warn!("Task panicked: {}", e);
                }
            }
        }

        optimizer.update(&evaluations);

        if let Some(checkpoint) = optimizer.checkpoint() {
            let tmp = checkpoint_path.with_extension("json.tmp");
            let bytes = serde_json::to_vec_pretty(&checkpoint)?;
            std::fs::write(&tmp, bytes).with_context(|| {
                format!("failed to write optimizer checkpoint {}", tmp.display())
            })?;
            std::fs::rename(&tmp, &checkpoint_path).with_context(|| {
                format!("failed to publish optimizer checkpoint {}", checkpoint_path.display())
            })?;
        }
    }

    // Flush store
    store.flush()?;
    manifest.add_artifact(output_dir.join("evaluations.jsonl"))?;
    manifest.write_atomic(&manifest_path)?;
    manifest.write_atomic(&root_manifest_path)?;

    // Print summary
    let all = store.load_all()?;
    let feasible: Vec<_> = all.iter().filter(|e| e.score.feasible()).collect();

    info!(
        "Campaign complete. Total evaluations: {}, Feasible: {}",
        all.len(),
        feasible.len()
    );

    if !feasible.is_empty() {
        let mut sorted = feasible.clone();
        sorted.sort_by(|a, b| b.score.value().partial_cmp(&a.score.value()).unwrap());

        info!("Top 5 pathological workloads:");
        for (i, eval) in sorted.iter().take(5).enumerate() {
            info!(
                "  {}: score={:.4} genome={:?}",
                i + 1,
                eval.score.value(),
                eval.genome.values
            );
        }
    }

    Ok(())
}

fn analyze(campaign_dir: PathBuf, top: usize) -> anyhow::Result<()> {
    let evaluations = CampaignStore::load_from_jsonl(campaign_dir.join("evaluations.jsonl"))?;

    let feasible: Vec<_> = evaluations.iter().filter(|e| e.score.feasible()).collect();

    println!("Total evaluations: {}", evaluations.len());
    println!("Feasible: {}", feasible.len());

    if !feasible.is_empty() {
        let mut sorted = feasible;
        sorted.sort_by(|a, b| b.score.value().partial_cmp(&a.score.value()).unwrap());

        println!("\nTop {} pathological workloads:", top);
        for (i, eval) in sorted.iter().take(top).enumerate() {
            println!("  {}: score={:.4}", i + 1, eval.score.value());
            for (key, val) in &eval.genome.values {
                println!("    {}: {}", key, val);
            }
        }
    }

    Ok(())
}
