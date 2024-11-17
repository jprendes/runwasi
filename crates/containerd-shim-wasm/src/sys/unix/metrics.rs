use shimkit::types::prost::Any;
use shimkit::types::{Result, Status};

#[cfg_attr(feature = "tracing", tracing::instrument(parent = tracing::Span::current(), skip_all, level = "Info"))]
pub fn get_metrics(_pid: u32) -> Result<Any> {
    /*
    let metrics = containerd_shim::cgroup::collect_metrics(pid)?;
    let metrics = containerd_shim::util::convert_to_any(Box::new(metrics))?;
    Ok(metrics)
    */
    Err(Status::unimplemented("metrics are not yet implemented"))
}
