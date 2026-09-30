//! Second reference body, deliberately unlike the flat: a valve filling a
//! cup. Its payloads are flow rates, not motion, and its views have no rooms.
//! Liquid above the cup's capacity spills; the effect bound is "no spill".
use super::sim::{contract, SimBodyV1};
use crate::error::AppResult;
use crate::physical::{contracts::*, require, values::*};
use serde::Deserialize;
use serde_json::{json, Value};

const CAPACITY_ML: f64 = 250.0;
const PAYLOAD_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["flowMlps"],"properties":{"flowMlps":{"type":"number","minimum":0}}}"#;
const READY_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["maxMl"],"properties":{"maxMl":{"minimum":0}}}"#;
const FILLED_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["dwellUs","minMl"],"properties":{"dwellUs":{"type":"integer","minimum":1},"minMl":{"minimum":0}}}"#;
const NO_SPILL_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FlowV1 {
    flow_mlps: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReadyV1 {
    max_ml: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct FilledV1 {
    min_ml: f64,
    dwell_us: u64,
}
/// The witness's measurement schema.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CupV1 {
    frame: String,
    cup_ml: f64,
    flow_mlps: f64,
    spilled_ml: f64,
}

fn round(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

pub(in crate::physical) struct DispenserBodyV1 {
    cup_ml: f64,
    flow_mlps: f64,
    spilled_ml: f64,
}

impl SimBodyV1 for DispenserBodyV1 {
    const KIND: &'static str = "sim.cup";
    const SUBSYSTEM: &'static str = "valve";
    const PAYLOAD_SCHEMA: &'static str = PAYLOAD_SCHEMA;
    const MIN_DECISION_INTERVAL_US: u64 = 250_000;
    fn options() -> Vec<(&'static str, Value)> {
        vec![
            ("idle", json!({"flowMlps": 0})),
            ("pour_large", json!({"flowMlps": 20})),
            ("pour_small", json!({"flowMlps": 5})),
        ]
    }
    fn observation_fields() -> Vec<&'static str> {
        vec!["/capacityMl", "/cupMl", "/flowing"]
    }
    fn bounds() -> AppResult<BoundSetV1> {
        BoundSetV1::try_from(vec![BoundV1 {
            pointer: JsonPointerV1::try_from("/flowMlps".to_owned())?,
            kind: BoundKindV1::AbsMax(NonNegative::try_from(20.0)?),
        }])
    }
    fn start_contract() -> AppResult<ContractRefV1> {
        contract("sim.cup.ready/v1", READY_SCHEMA, json!({"maxMl": 200}))
    }
    fn completion_contract() -> AppResult<ContractRefV1> {
        contract(
            "sim.cup.filled/v1",
            FILLED_SCHEMA,
            json!({"minMl": 40, "dwellUs": 200_000}),
        )
    }
    fn effect_contract() -> AppResult<ContractRefV1> {
        contract("sim.cup.no-spill/v1", NO_SPILL_SCHEMA, json!({}))
    }
    fn handover_contract() -> AppResult<ContractRefV1> {
        contract("sim.cup.valve-closed/v1", NO_SPILL_SCHEMA, json!({}))
    }

    fn initial() -> Self {
        Self {
            cup_ml: 0.0,
            flow_mlps: 0.0,
            spilled_ml: 0.0,
        }
    }
    fn advance(&mut self, command: Option<&Value>, dt_us: u64) {
        self.flow_mlps = command
            .and_then(|c| serde_json::from_value::<FlowV1>(c.clone()).ok())
            .map_or(0.0, |f| f.flow_mlps);
        let filled = self.cup_ml + self.flow_mlps * dt_us as f64 / 1e6;
        self.spilled_ml += (filled - CAPACITY_ML).max(0.0);
        self.cup_ml = filled.min(CAPACITY_ML);
    }
    fn moving(&self) -> bool {
        self.flow_mlps != 0.0
    }
    fn ready(&self, start: &ContractRefV1) -> AppResult<()> {
        let want: ReadyV1 = start.params.decode()?;
        require(self.cup_ml <= want.max_ml, "Cup too full to start")
    }
    fn view(&self) -> Value {
        json!({
            "cupMl": round(self.cup_ml),
            "flowing": self.flow_mlps != 0.0,
            "capacityMl": CAPACITY_ML,
        })
    }
    fn measurement(&self) -> Value {
        json!({
            "frame": "cup",
            "cupMl": round(self.cup_ml),
            "flowMlps": round(self.flow_mlps),
            "spilledMl": round(self.spilled_ml),
        })
    }

    fn complete(contract: &ContractRefV1, m: &Value) -> AppResult<bool> {
        let want: FilledV1 = contract.params.decode()?;
        let c: CupV1 = serde_json::from_value(m.clone())?;
        require(c.frame == "cup", "Foreign measurement frame")?;
        Ok(c.cup_ml >= want.min_ml && c.flow_mlps == 0.0)
    }
    fn dwell_us(contract: &ContractRefV1) -> AppResult<u64> {
        Ok(contract.params.decode::<FilledV1>()?.dwell_us)
    }
    fn at_rest(m: &Value) -> AppResult<bool> {
        let c: CupV1 = serde_json::from_value(m.clone())?;
        require(c.frame == "cup", "Foreign measurement frame")?;
        Ok(c.flow_mlps == 0.0)
    }
    fn within_bound(_: &ContractRefV1, m: &Value) -> AppResult<bool> {
        let c: CupV1 = serde_json::from_value(m.clone())?;
        require(c.frame == "cup", "Foreign measurement frame")?;
        Ok(c.spilled_ml == 0.0)
    }
}
