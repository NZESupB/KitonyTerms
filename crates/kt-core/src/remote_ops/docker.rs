//! Docker CLI 的限定字段查询与类型化采样；不读取容器环境或命令行。

use serde::de::DeserializeOwned;
use serde::Deserialize;

use super::{shell_quote, validate_resource_id, OperationsError, OperationsErrorKind};

#[derive(Debug, Clone, PartialEq)]
pub struct DockerRuntimeUsage {
    pub container_id: String,
    pub cpu_percent: Option<f64>,
    pub memory_used_bytes: Option<u64>,
    pub memory_limit_bytes: Option<u64>,
    pub memory_percent: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DockerContainerStorage {
    #[serde(rename = "id")]
    pub container_id: String,
    pub image_id: String,
    #[serde(default, deserialize_with = "optional_size")]
    pub size_rw: Option<u64>,
    #[serde(default, deserialize_with = "optional_size")]
    pub size_rootfs: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct DockerImageSize {
    #[serde(rename = "id")]
    pub image_id: String,
    #[serde(default, deserialize_with = "optional_size")]
    pub size: Option<u64>,
}

fn optional_size<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<u64>, D::Error> {
    let value = Option::<i64>::deserialize(d)?;
    match value {
        None | Some(-1) => Ok(None),
        Some(value) if value >= 0 => Ok(Some(value as u64)),
        _ => Err(serde::de::Error::custom("Docker 大小数值无效")),
    }
}

pub(super) fn inspect_batch(ids: &[String], images: bool) -> Result<String, OperationsError> {
    if ids.is_empty() || ids.len() > 64 {
        return Err(OperationsError::new(
            OperationsErrorKind::ParseFailed,
            "Docker 查询批次数量无效",
        ));
    }
    let targets = ids
        .iter()
        .map(|id| validate_resource_id(id).map(shell_quote))
        .collect::<Result<Vec<_>, _>>()?
        .join(" ");
    let (resource, projection) = if images {
        ("image", r#"{"id":{{json .Id}},"size":{{json .Size}}}"#)
    } else {
        (
            "container",
            r#"{"id":{{json .Id}},"image_id":{{json .Image}},"size_rw":{{json .SizeRw}},"size_rootfs":{{json .SizeRootFs}}}"#,
        )
    };
    let size_flag = if images { "" } else { "--size " };
    Ok(format!(
        "docker {resource} inspect {size_flag}--format {} {targets}",
        shell_quote(projection)
    ))
}

pub(super) fn parse_lines<T: DeserializeOwned>(stdout: &str) -> Result<Vec<T>, OperationsError> {
    stdout
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| {
            serde_json::from_str(line).map_err(|_| OperationsError::parse("Docker JSON 解析失败"))
        })
        .collect()
}

#[derive(Deserialize)]
struct StatsRow {
    #[serde(rename = "ID")]
    id: String,
    #[serde(rename = "CPUPerc", default)]
    cpu: String,
    #[serde(rename = "MemUsage", default)]
    memory: String,
    #[serde(rename = "MemPerc", default)]
    memory_percent: String,
}

fn percent(raw: &str) -> Result<Option<f64>, OperationsError> {
    let raw = raw.trim();
    if matches!(raw, "" | "--") {
        return Ok(None);
    }
    let value = raw
        .strip_suffix('%')
        .ok_or_else(|| OperationsError::parse("Docker 百分比格式无效"))?
        .trim()
        .parse::<f64>()
        .map_err(|_| OperationsError::parse("Docker 百分比解析失败"))?;
    if !value.is_finite() || value < 0.0 {
        return Err(OperationsError::parse("Docker 百分比数值无效"));
    }
    Ok(Some(value))
}

fn bytes(raw: &str) -> Result<Option<u64>, OperationsError> {
    let raw = raw.trim();
    if matches!(raw, "" | "--") {
        return Ok(None);
    }
    let split = raw
        .find(|c: char| !(c.is_ascii_digit() || c == '.'))
        .ok_or_else(|| OperationsError::parse("Docker 内存单位缺失"))?;
    let value = raw[..split]
        .parse::<f64>()
        .map_err(|_| OperationsError::parse("Docker 内存数值无效"))?;
    let factor = match raw[split..].trim() {
        "B" => 1.0,
        "kB" | "KB" => 1e3,
        "MB" => 1e6,
        "GB" => 1e9,
        "TB" => 1e12,
        "KiB" => 1024.0,
        "MiB" => 1024f64.powi(2),
        "GiB" => 1024f64.powi(3),
        "TiB" => 1024f64.powi(4),
        _ => return Err(OperationsError::parse("Docker 内存单位无效")),
    };
    let result = value * factor;
    if !result.is_finite() || result < 0.0 || result >= u64::MAX as f64 {
        return Err(OperationsError::parse("Docker 内存数值越界"));
    }
    Ok(Some(result.round() as u64))
}

pub fn parse_docker_stats(stdout: &str) -> Result<Vec<DockerRuntimeUsage>, OperationsError> {
    parse_lines::<StatsRow>(stdout)?
        .into_iter()
        .map(|row| {
            validate_resource_id(&row.id)?;
            let (used, limit) = if matches!(row.memory.trim(), "" | "--") {
                (None, None)
            } else {
                let (used, limit) = row
                    .memory
                    .split_once('/')
                    .ok_or_else(|| OperationsError::parse("Docker 内存用量格式无效"))?;
                (bytes(used)?, bytes(limit)?)
            };
            Ok(DockerRuntimeUsage {
                container_id: row.id,
                cpu_percent: percent(&row.cpu)?,
                memory_used_bytes: used,
                memory_limit_bytes: limit,
                memory_percent: percent(&row.memory_percent)?,
            })
        })
        .collect()
}

pub fn parse_docker_storage(stdout: &str) -> Result<Vec<DockerContainerStorage>, OperationsError> {
    let rows = parse_lines::<DockerContainerStorage>(stdout)?;
    for row in &rows {
        validate_resource_id(&row.container_id)?;
        validate_resource_id(&row.image_id)?;
    }
    Ok(rows)
}

pub fn parse_docker_image_sizes(stdout: &str) -> Result<Vec<DockerImageSize>, OperationsError> {
    let rows = parse_lines::<DockerImageSize>(stdout)?;
    for row in &rows {
        validate_resource_id(&row.image_id)?;
    }
    Ok(rows)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_preserve_multicore_cpu_and_cli_memory_units() {
        let rows = parse_docker_stats(
            r#"{"ID":"abc","CPUPerc":"234.5%","MemUsage":"1.5MiB / 2GiB","MemPerc":"0.1%"}
{"ID":"def","CPUPerc":"--","MemUsage":"--","MemPerc":"--"}"#,
        )
        .unwrap();
        assert_eq!(rows[0].cpu_percent, Some(234.5));
        assert_eq!(rows[0].memory_used_bytes, Some(1572864));
        assert_eq!(rows[0].memory_limit_bytes, Some(2147483648));
        assert_eq!(rows[1].cpu_percent, None);
        assert_eq!(bytes("1.25GB").unwrap(), Some(1250000000));
        assert!(parse_docker_stats("bad json").is_err());
        assert!(percent("NaN%").is_err());
        assert!(bytes("-1MiB").is_err());
        assert!(bytes("2unknown").is_err());
        assert!(parse_docker_stats("").unwrap().is_empty());
    }

    #[test]
    fn storage_keeps_unknown_distinct_from_zero_and_rejects_invalid_sizes() {
        let row = &parse_docker_storage(
            r#"{"id":"abc","image_id":"sha256:def","size_rw":0,"size_rootfs":-1}"#,
        )
        .unwrap()[0];
        assert_eq!(row.size_rw, Some(0));
        assert_eq!(row.size_rootfs, None);
        assert!(parse_docker_storage(r#"{"id":"abc","image_id":"def","size_rw":-2}"#).is_err());
        assert!(inspect_batch(&[], false).is_err());
        assert!(inspect_batch(&vec!["abc".into(); 65], false).is_err());
        assert!(inspect_batch(&["a;echo bad".into()], false).is_err());
        let command = inspect_batch(&["abc".into()], false).unwrap();
        assert!(command.contains("--size --format"));
        assert!(!command.contains(".Config"));
        assert!(inspect_batch(&["sha256:abc".into()], true)
            .unwrap()
            .contains("docker image inspect"));
    }
}
