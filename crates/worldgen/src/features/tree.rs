//! Trees and huge mushrooms.

use super::provider::StateProvider;
use super::{Features, R};
use crate::region::Region;
use anyhow::{bail, Result};
use serde_json::Value;
use std::sync::Arc;

pub struct Tree {}

impl Tree {
    pub fn parse(_f: &mut Features, _c: &Value) -> Result<Tree> { bail!("trees not implemented") }
    pub fn place(&self, _f: &Features, _reg: &mut Region, _r: &mut R, _x: i32, _y: i32, _z: i32) -> bool { false }
}

#[allow(clippy::too_many_arguments)]
pub fn huge_mushroom(_f: &Features, _red: bool, _cap: &StateProvider, _stem: &StateProvider, _radius: i32, _on: &Arc<Vec<bool>>, _reg: &mut Region, _r: &mut R, _x: i32, _y: i32, _z: i32) -> bool {
    false
}
