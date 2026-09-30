//! Reference body: a disk moving in a two-room flat, unrelated to any real
//! device. 2D kinematics only: the body turns in place or moves along its
//! heading, and walls block it. A wall divides the living room (west) from
//! the bedroom (east); one door in that wall joins them.
//!
//! Options are fixed commands (speed along the heading, turn rate). The brain
//! view carries the room, a hint toward the bedroom door and the pose; the
//! witness reads the pose from its own measurements.
use super::sim::{contract, SimBodyV1};
use crate::error::AppResult;
use crate::physical::{contracts::*, require, values::*};
use serde::Deserialize;
use serde_json::{json, Value};
use std::f64::consts::{FRAC_PI_2, PI};

/// Flat extent and wall geometry, metres.
const WIDTH: f64 = 10.0;
const DEPTH: f64 = 4.0;
const WALL_X: f64 = 5.0;
const WALL_HALF: f64 = 0.05;
const DOOR: (f64, f64) = (1.4, 2.6);
const RADIUS: f64 = 0.15;
/// Where the body starts, and the point in the bedroom the hint aims at.
const START: (f64, f64, f64) = (2.0, 1.2, FRAC_PI_2);
const TARGET: (f64, f64) = (6.0, 2.0);
/// Heading error below which the hint says "ahead".
const HINT_TOLERANCE: f64 = 0.2;

const PAYLOAD_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["speedMps","turnRadps"],"properties":{"speedMps":{"type":"number"},"turnRadps":{"type":"number"}}}"#;
const ROOM_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["room"],"properties":{"room":{"enum":["living_room","bedroom"]}}}"#;
const IN_ROOM_SCHEMA: &str = r#"{"type":"object","additionalProperties":false,"required":["dwellUs","room"],"properties":{"dwellUs":{"type":"integer","minimum":1},"room":{"enum":["living_room","bedroom"]}}}"#;
const INSIDE_SCHEMA: &str = r#"{"type":"object","additionalProperties":false}"#;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CommandV1 {
    speed_mps: f64,
    turn_radps: f64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RoomV1 {
    room: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct InRoomV1 {
    room: String,
    dwell_us: u64,
}
/// The witness's measurement schema.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PoseV1 {
    frame: String,
    x: f64,
    y: f64,
    heading: f64,
    speed_mps: f64,
}

fn wrap(angle: f64) -> f64 {
    let a = (angle + PI).rem_euclid(2.0 * PI) - PI;
    if a <= -PI {
        a + 2.0 * PI
    } else {
        a
    }
}
fn round(v: f64) -> f64 {
    let r = (v * 1000.0).round() / 1000.0;
    if r == 0.0 {
        0.0
    } else {
        r
    }
}
/// Whether a disk at (x, y) is clear of the outer walls and the dividing wall.
fn clear(x: f64, y: f64) -> bool {
    let inside =
        x - RADIUS >= 0.0 && x + RADIUS <= WIDTH && y - RADIUS >= 0.0 && y + RADIUS <= DEPTH;
    let at_wall = (x - WALL_X).abs() < WALL_HALF + RADIUS;
    let in_door = y - RADIUS >= DOOR.0 && y + RADIUS <= DOOR.1;
    inside && (!at_wall || in_door)
}
/// The room the whole disk is in, or the doorway while it overlaps the wall.
fn room(x: f64) -> &'static str {
    if x + RADIUS < WALL_X - WALL_HALF {
        "living_room"
    } else if x - RADIUS > WALL_X + WALL_HALF {
        "bedroom"
    } else {
        "doorway"
    }
}

pub(in crate::physical) struct FlatBodyV1 {
    x: f64,
    y: f64,
    heading: f64,
    speed: f64,
    blocked: bool,
}
impl FlatBodyV1 {
    /// Back at the start pose, at rest (someone carried it there).
    pub(in crate::physical) fn place_at_start(&mut self) {
        *self = Self::initial();
    }
    fn hint(&self) -> &'static str {
        let bearing = (TARGET.1 - self.y).atan2(TARGET.0 - self.x);
        let error = wrap(bearing - self.heading);
        if error > HINT_TOLERANCE {
            "left"
        } else if error < -HINT_TOLERANCE {
            "right"
        } else {
            "ahead"
        }
    }
}

impl SimBodyV1 for FlatBodyV1 {
    const KIND: &'static str = "sim.flat";
    const SUBSYSTEM: &'static str = "base";
    const PAYLOAD_SCHEMA: &'static str = PAYLOAD_SCHEMA;
    const MIN_DECISION_INTERVAL_US: u64 = 100_000;
    fn options() -> Vec<(&'static str, Value)> {
        let command = |speed: f64, turn: f64| json!({"speedMps": speed, "turnRadps": turn});
        vec![
            ("forward", command(1.0, 0.0)),
            ("sprint", command(3.0, 0.0)),
            ("stop", command(0.0, 0.0)),
            ("turn_left", command(0.0, FRAC_PI_2)),
            ("turn_right", command(0.0, -FRAC_PI_2)),
        ]
    }
    fn observation_fields() -> Vec<&'static str> {
        vec![
            "/blocked",
            "/heading",
            "/headingToGoal",
            "/room",
            "/speedMps",
            "/x",
            "/y",
        ]
    }
    fn bounds() -> AppResult<BoundSetV1> {
        let abs = |pointer: &str, max: f64| -> AppResult<BoundV1> {
            Ok(BoundV1 {
                pointer: JsonPointerV1::try_from(pointer.to_owned())?,
                kind: BoundKindV1::AbsMax(NonNegative::try_from(max)?),
            })
        };
        BoundSetV1::try_from(vec![abs("/speedMps", 3.0)?, abs("/turnRadps", FRAC_PI_2)?])
    }
    fn start_contract() -> AppResult<ContractRefV1> {
        contract(
            "sim.flat.at-rest-in-room/v1",
            ROOM_SCHEMA,
            json!({"room": "living_room"}),
        )
    }
    fn completion_contract() -> AppResult<ContractRefV1> {
        contract(
            "sim.flat.in-room/v1",
            IN_ROOM_SCHEMA,
            json!({"room": "bedroom", "dwellUs": 200_000}),
        )
    }
    fn effect_contract() -> AppResult<ContractRefV1> {
        contract("sim.flat.inside/v1", INSIDE_SCHEMA, json!({}))
    }
    fn handover_contract() -> AppResult<ContractRefV1> {
        contract("sim.flat.at-rest/v1", INSIDE_SCHEMA, json!({}))
    }

    fn initial() -> Self {
        Self {
            x: START.0,
            y: START.1,
            heading: START.2,
            speed: 0.0,
            blocked: false,
        }
    }
    fn advance(&mut self, command: Option<&Value>, dt_us: u64) {
        let dt = dt_us as f64 / 1e6;
        let c = command
            .and_then(|c| serde_json::from_value::<CommandV1>(c.clone()).ok())
            .unwrap_or(CommandV1 {
                speed_mps: 0.0,
                turn_radps: 0.0,
            });
        self.heading = wrap(self.heading + c.turn_radps * dt);
        let (x, y) = (
            self.x + c.speed_mps * self.heading.cos() * dt,
            self.y + c.speed_mps * self.heading.sin() * dt,
        );
        self.blocked = c.speed_mps != 0.0 && !clear(x, y);
        if self.blocked {
            self.speed = 0.0;
        } else {
            (self.x, self.y, self.speed) = (x, y, c.speed_mps);
        }
    }
    fn moving(&self) -> bool {
        self.speed != 0.0
    }
    fn ready(&self, start: &ContractRefV1) -> AppResult<()> {
        let want: RoomV1 = start.params.decode()?;
        require(
            room(self.x) == want.room,
            "Body is not in the starting room",
        )
    }
    fn view(&self) -> Value {
        json!({
            "room": room(self.x),
            "headingToGoal": self.hint(),
            "x": round(self.x),
            "y": round(self.y),
            "heading": round(self.heading),
            "speedMps": round(self.speed),
            "blocked": self.blocked,
        })
    }
    fn measurement(&self) -> Value {
        json!({
            "frame": "flat",
            "x": round(self.x),
            "y": round(self.y),
            "heading": round(self.heading),
            "speedMps": round(self.speed),
        })
    }

    fn complete(contract: &ContractRefV1, m: &Value) -> AppResult<bool> {
        let want: InRoomV1 = contract.params.decode()?;
        let p: PoseV1 = serde_json::from_value(m.clone())?;
        require(p.frame == "flat", "Foreign measurement frame")?;
        Ok(room(p.x) == want.room && p.speed_mps == 0.0 && p.heading.is_finite())
    }
    fn dwell_us(contract: &ContractRefV1) -> AppResult<u64> {
        Ok(contract.params.decode::<InRoomV1>()?.dwell_us)
    }
    fn at_rest(m: &Value) -> AppResult<bool> {
        let p: PoseV1 = serde_json::from_value(m.clone())?;
        require(p.frame == "flat", "Foreign measurement frame")?;
        Ok(p.speed_mps == 0.0)
    }
    fn within_bound(_: &ContractRefV1, m: &Value) -> AppResult<bool> {
        let p: PoseV1 = serde_json::from_value(m.clone())?;
        require(p.frame == "flat", "Foreign measurement frame")?;
        Ok(p.x - RADIUS >= 0.0
            && p.x + RADIUS <= WIDTH
            && p.y - RADIUS >= 0.0
            && p.y + RADIUS <= DEPTH)
    }
}
