use std::{
    fmt::{Display, Write},
    str::FromStr,
    time::Instant,
};

use anyhow::{Context, bail};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, BufReader, Lines};

#[derive(Debug, Clone, Copy)]
struct RegisterGroup {
    start_address: u16,
    length: u16,
}

pub struct MetricStore {
    metrics: Vec<Metric>,
    reg_groups: Vec<RegisterGroup>,
    reg_table: Vec<u16>,
}
#[derive(Debug)]
struct Metric {
    name: String,
    desc: String,
    metric_type: MetricType,
    data_type: DataType,
    content: Content,
    scale: u16,
}

#[derive(Debug)]
enum MetricType {
    Gauge,
    Counter,
}

impl Display for MetricType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MetricType::Gauge => write!(f, "gauge"),
            MetricType::Counter => write!(f, "counter"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum DataType {
    Short,
    Ushort,
    Ulong,
}

#[derive(Debug)]
enum Content {
    Single {
        reg: u16,
    },
    Many {
        label_name: String,
        label_values: Vec<(String, u16)>,
    },
}

impl MetricStore {
    pub async fn init_from_regmap(regmap_source: impl AsyncRead + Unpin) -> anyhow::Result<Self> {
        let mut lines = BufReader::new(regmap_source).lines();
        let mut regs: Vec<u16> = Vec::new();
        let mut metrics: Vec<Metric> = Vec::new();
        let mut line_num = 0;

        let pre = Instant::now();
        while let Some(metric) = read_metric(&mut lines, &mut regs, &mut line_num)
            .await
            .with_context(|| format!("error while parsing regmap at line {line_num}"))?
        {
            metrics.push(metric);
        }
        regs.sort_unstable();
        let reg_table = vec![0; regs[regs.len() - 1] as usize + 1];
        let reg_groups = group_registers(&regs);
        tracing::debug!(
            "loaded {} metrics in {} registers",
            metrics.len(),
            regs.len()
        );
        tracing::debug!("register groups: {reg_groups:?}");
        tracing::debug!(
            "parsing regmap took {}ms",
            pre.elapsed().as_secs_f64() * 1000.0
        );
        Ok(Self {
            metrics,
            reg_groups,
            reg_table,
        })
    }

    pub async fn update_from_solarman(
        &mut self,
        solarman: &mut solarman_tokio::Client,
    ) -> anyhow::Result<()> {
        for group in &self.reg_groups {
            let reg_values = solarman
                .read_holding_registers(group.start_address, group.length)
                .await?;
            for (offset, val) in reg_values.iter().enumerate() {
                self.reg_table[group.start_address as usize + offset] = *val;
            }
        }
        Ok(())
    }

    fn value_writeln(&self, f: &mut impl Write, data_type: DataType, base_addr: u16, scale: u16) {
        let addr = base_addr as usize;
        let num = match data_type {
            DataType::Short => i64::from(self.reg_table[addr].cast_signed()),
            DataType::Ushort => i64::from(self.reg_table[addr]),
            DataType::Ulong => {
                i64::from(self.reg_table[addr]) | (i64::from(self.reg_table[addr + 1]) << 16)
            }
        };
        if scale == 1 {
            return writeln!(f, "{num}").unwrap();
        }
        let scale = i64::from(scale);
        let whole = num / scale;
        let dec = num.abs() % scale;
        let pad = scale.ilog10() as usize;
        writeln!(f, "{whole}.{dec:0pad$}").unwrap();
    }

    pub fn encode_prometheus(&self) -> String {
        let mut out = String::new();
        let pre = Instant::now();
        for metric in &self.metrics {
            writeln!(
                out,
                "# HELP {} {}\n# TYPE {} {}",
                metric.name, metric.desc, metric.name, metric.metric_type
            )
            .unwrap();
            match &metric.content {
                Content::Single { reg } => {
                    write!(out, "{} ", metric.name).unwrap();
                    self.value_writeln(&mut out, metric.data_type, *reg, metric.scale);
                }
                Content::Many {
                    label_name,
                    label_values,
                } => {
                    for (label_value, reg) in label_values {
                        write!(out, "{}{{{label_name}=\"{label_value}\"}} ", metric.name).unwrap();
                        self.value_writeln(&mut out, metric.data_type, *reg, metric.scale);
                    }
                }
            }
            writeln!(out).unwrap();
        }
        tracing::debug!(
            "encoding metrics took {}ms",
            pre.elapsed().as_secs_f64() * 1000.0
        );
        out
    }
}

impl FromStr for MetricType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "gauge" => Self::Gauge,
            "counter" => Self::Counter,
            o => bail!("{o} is not a valid metric type"),
        })
    }
}

impl FromStr for DataType {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "short" => Self::Short,
            "ushort" => Self::Ushort,
            "ulong" => Self::Ulong,
            o => bail!("{o} is not a valid data type"),
        })
    }
}

async fn read_metric<T: AsyncBufRead + Unpin>(
    lines: &mut Lines<T>,
    regs: &mut Vec<u16>,
    line_num: &mut usize,
) -> anyhow::Result<Option<Metric>> {
    loop {
        let Some(line) = lines.next_line().await? else {
            return Ok(None);
        };
        *line_num += 1;
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }

        let Some((props, desc)) = line.split_once("--") else {
            bail!("metric must contain description after --");
        };
        let mut props = props.split_whitespace();

        let Some(data_type) = props.next() else {
            bail!("no metric data type");
        };
        let data_type = DataType::from_str(data_type)?;

        let Some(reg) = props.next() else {
            bail!("no metric register addr");
        };

        let Some(name) = props.next() else {
            bail!("no metric name");
        };

        let Some(metric_type) = props.next() else {
            bail!("no metric type");
        };
        let metric_type = MetricType::from_str(metric_type)?;

        let scale = if let Some(scale) = props.next() {
            let Some(scale) = scale.strip_prefix('/') else {
                bail!("invalid scale format");
            };
            scale.parse()?
        } else {
            1
        };

        let content = if reg.starts_with('[') && reg.ends_with(']') {
            let label_name = reg[1..reg.len() - 1].to_string();
            let mut label_values = Vec::new();
            loop {
                let Some(line) = lines.next_line().await? else {
                    bail!("unexpected end of file (unclosed group metric)");
                };
                *line_num += 1;
                let line = line.trim();
                if line == "end" {
                    break;
                }
                let Some((reg, value)) = line.split_once(char::is_whitespace) else {
                    bail!("invalid label format {line:?}");
                };
                let reg = reg.parse()?;
                regs.push(reg);
                label_values.push((value.into(), reg));
            }
            Content::Many {
                label_name,
                label_values,
            }
        } else {
            let reg = reg.parse()?;
            regs.push(reg);
            Content::Single { reg }
        };

        return Ok(Some(Metric {
            name: name.into(),
            desc: desc.trim().into(),
            metric_type,
            data_type,
            content,
            scale,
        }));
    }
}

fn group_registers(regs: &[u16]) -> Vec<RegisterGroup> {
    let mut groups = Vec::new();
    let mut current_group: Option<RegisterGroup> = None;

    for reg in regs {
        let reg = *reg;
        if let Some(group) = &mut current_group {
            // check if this metric can be added to the current group
            let group_end = group.start_address + group.length;
            if reg >= group_end && reg <= group_end + 10 {
                // allow up to 10 reg gaps for less read requests
                if reg > group_end {
                    group.length += reg - group_end;
                }
                group.length += 1;
            } else {
                groups.push(current_group.take().unwrap());
                current_group = Some(RegisterGroup {
                    start_address: reg,
                    length: 1,
                });
            }
        } else {
            current_group = Some(RegisterGroup {
                start_address: reg,
                length: 1,
            });
        }
    }

    if let Some(group) = current_group {
        groups.push(group);
    }

    groups
}
