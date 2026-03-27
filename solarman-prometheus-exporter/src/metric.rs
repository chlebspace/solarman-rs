use std::{fmt::Display, marker::PhantomData, str::FromStr};

use anyhow::bail;
use serde::{
    Deserialize, Deserializer,
    de::{self, MapAccess, SeqAccess, Visitor},
};

#[derive(Debug)]
pub struct Metric {
    pub name: String,
    pub desc: String,
    pub metric_type: MetricType,
    pub signed: bool,
    pub regs: MetricContent,
    pub mapping: Vec<MappingOperation>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MetricType {
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
pub struct ModbusCell(pub u16, pub Option<u16>);

struct ModbusCellVisitor;

impl<'de> Visitor<'de> for ModbusCellVisitor {
    type Value = ModbusCell;

    fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
        formatter.write_str("a u16 or a two-element [u16, u16] array")
    }

    fn visit_i64<E>(self, v: i64) -> Result<Self::Value, E>
    where
        E: de::Error,
    {
        Ok(ModbusCell(v as u16, None))
    }

    fn visit_seq<A>(self, mut seq: A) -> Result<Self::Value, A::Error>
    where
        A: SeqAccess<'de>,
    {
        let low: u16 = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(0, &self))?;
        let high: u16 = seq
            .next_element()?
            .ok_or_else(|| de::Error::invalid_length(1, &self))?;
        if seq.next_element::<u16>()?.is_some() {
            return Err(de::Error::invalid_length(3, &self));
        }
        Ok(ModbusCell(low, Some(high)))
    }
}

impl<'de> Deserialize<'de> for ModbusCell {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        deserializer.deserialize_any(ModbusCellVisitor)
    }
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum MetricContent {
    Single(ModbusCell),
    Many {
        label: String,
        #[serde(deserialize_with = "deserialize_map_to_vec")]
        values: Vec<(String, ModbusCell)>,
    },
}

fn deserialize_map_to_vec<'de, K, V, D>(deserializer: D) -> Result<Vec<(K, V)>, D::Error>
where
    K: Deserialize<'de>,
    V: Deserialize<'de>,
    D: Deserializer<'de>,
{
    struct MapToVecVisitor<K, V>(PhantomData<(K, V)>);

    impl<'de, K, V> Visitor<'de> for MapToVecVisitor<K, V>
    where
        K: Deserialize<'de>,
        V: Deserialize<'de>,
    {
        type Value = Vec<(K, V)>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a map")
        }

        fn visit_map<M>(self, mut access: M) -> Result<Self::Value, M::Error>
        where
            M: MapAccess<'de>,
        {
            let mut vec = Vec::with_capacity(access.size_hint().unwrap_or(0));
            while let Some((key, value)) = access.next_entry()? {
                vec.push((key, value));
            }
            Ok(vec)
        }
    }

    deserializer.deserialize_map(MapToVecVisitor(PhantomData))
}

#[derive(Debug)]
pub enum MappingOperation {
    Add(f64),
    Multiply(f64),
}

impl FromStr for MappingOperation {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> anyhow::Result<Self> {
        let Some(op) = s.chars().next() else {
            bail!("empty string supplied");
        };
        let num: f64 = s[1..].parse()?;
        Ok(match op {
            '+' => Self::Add(num),
            '-' => Self::Add(-num),
            '*' => Self::Multiply(num),
            _ => bail!("unknown operation {op}"),
        })
    }
}

fn deserialize_mapping<'de, D>(deserializer: D) -> Result<Vec<MappingOperation>, D::Error>
where
    D: Deserializer<'de>,
{
    let s: &str = Deserialize::deserialize(deserializer)?;
    let mut ops = Vec::new();
    for raw_op in s.split_ascii_whitespace() {
        ops.push(MappingOperation::from_str(raw_op).map_err(serde::de::Error::custom)?);
    }
    Ok(ops)
}

#[derive(Deserialize)]
struct MetricFields {
    #[serde(rename = "type")]
    metric_type: MetricType,
    desc: String,
    #[serde(deserialize_with = "deserialize_metric_content")]
    regs: MetricContent,
    #[serde(default)]
    signed: bool,
    #[serde(default, deserialize_with = "deserialize_mapping")]
    map: Vec<MappingOperation>,
}

fn deserialize_metric_content<'de, D>(deserializer: D) -> Result<MetricContent, D::Error>
where
    D: Deserializer<'de>,
{
    MetricContent::deserialize(deserializer)
}

pub fn load_metrics(toml_str: &str) -> anyhow::Result<Vec<Metric>> {
    struct VecMetricVisitor;

    impl<'de> Visitor<'de> for VecMetricVisitor {
        type Value = Vec<Metric>;

        fn expecting(&self, formatter: &mut std::fmt::Formatter) -> std::fmt::Result {
            formatter.write_str("a TOML document with metric sections")
        }

        fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
        where
            A: de::MapAccess<'de>,
        {
            let mut metrics = Vec::new();
            while let Some(key) = map.next_key::<String>()? {
                let metric = map.next_value::<MetricFields>().map(|f| Metric {
                    name: key,
                    desc: f.desc,
                    metric_type: f.metric_type,
                    signed: f.signed,
                    regs: f.regs,
                    mapping: f.map,
                })?;
                metrics.push(metric);
            }
            Ok(metrics)
        }
    }

    Ok(toml::Deserializer::parse(toml_str)?.deserialize_map(VecMetricVisitor)?)
}
