use anyhow::Result;
use containerd_shim::cgroup::collect_metrics;
use containerd_shim::protos::prost_types::Any;

pub fn get_metrics(pid: u32) -> Result<Any> {
    let metrics = collect_metrics(pid)?;

    let metrics = Any::from_msg(&metrics)?;
    Ok(metrics)
}
