use std::{fmt::Write, time::Instant};

use crate::metric::{MappingOperation, Metric, MetricContent, ModbusCell};

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

impl MetricStore {
    pub fn create(regmap_source: &str) -> anyhow::Result<Self> {
        let pre = Instant::now();

        let metrics = crate::metric::load_metrics(regmap_source)?;
        tracing::debug!("metrics parsed: {metrics:#?}");

        let mut regs: Vec<u16> = Vec::new();
        for cells in metrics.iter().map(|m| &m.regs) {
            match cells {
                crate::metric::MetricContent::Single(ModbusCell(low, high)) => {
                    regs.push(*low);
                    if let Some(high) = high {
                        regs.push(*high);
                    }
                }
                crate::metric::MetricContent::Many {
                    label: _label_name,
                    values: label_values,
                } => {
                    for (_, ModbusCell(low, high)) in label_values.iter() {
                        regs.push(*low);
                        if let Some(high) = high {
                            regs.push(*high);
                        }
                    }
                }
            }
        }
        regs.sort_unstable();

        let reg_table = vec![0; regs[regs.len() - 1] as usize + 1];
        let reg_groups = group_registers(&regs);
        let elapsed = pre.elapsed().as_secs_f64() * 1000.0;
        tracing::debug!(
            "loaded {} metrics in {} registers",
            metrics.len(),
            regs.len()
        );
        tracing::debug!("register groups: {reg_groups:?}");
        tracing::debug!("parsing regmap took {elapsed}ms");
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

    fn writeln_cell_value(
        &self,
        f: &mut impl Write,
        cell: ModbusCell,
        signed: bool,
        mapping: &[MappingOperation],
    ) {
        let num: i64 = match cell {
            ModbusCell(low, None) if signed => {
                i64::from(self.reg_table[usize::from(low)].cast_signed())
            }
            ModbusCell(low, None) => i64::from(self.reg_table[usize::from(low)]),
            ModbusCell(low, Some(high)) => {
                let low_32 = u32::from(self.reg_table[usize::from(low)]);
                let high_32 = u32::from(self.reg_table[usize::from(high)]);
                let raw = low_32 | (high_32 << 16);
                if signed {
                    i64::from(raw.cast_signed())
                } else {
                    i64::from(raw)
                }
            }
        };
        if mapping.is_empty() {
            writeln!(f, "{num}").unwrap();
        } else {
            let num = mapping.iter().fold(num as f64, |n, m| match m {
                MappingOperation::Add(v) => n + v,
                MappingOperation::Multiply(v) => n * v,
            });
            writeln!(f, "{num}").unwrap();
        }
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
            match &metric.regs {
                MetricContent::Single(cell) => {
                    write!(out, "{} ", metric.name).unwrap();
                    self.writeln_cell_value(&mut out, *cell, metric.signed, &metric.mapping);
                }
                MetricContent::Many {
                    label: label_name,
                    values: label_values,
                } => {
                    for (label_value, cell) in label_values {
                        write!(out, "{}{{{label_name}=\"{label_value}\"}} ", metric.name).unwrap();
                        self.writeln_cell_value(&mut out, *cell, metric.signed, &metric.mapping);
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

fn group_registers(regs: &[u16]) -> Vec<RegisterGroup> {
    let mut groups = Vec::new();
    let mut current_group: Option<RegisterGroup> = None;

    for reg in regs {
        let reg = *reg;
        if let Some(group) = &mut current_group {
            let group_end = group.start_address + group.length;
            if reg >= group_end && reg <= group_end + 10 {
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
