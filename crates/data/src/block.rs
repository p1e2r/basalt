use crate::blocks::*;
use crate::BlockInfo;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct BlockState(pub u32);

fn name_index() -> &'static HashMap<&'static str, u16> {
    static M: OnceLock<HashMap<&'static str, u16>> = OnceLock::new();
    M.get_or_init(|| {
        BLOCKS
            .iter()
            .enumerate()
            .map(|(i, b)| (b.name, i as u16))
            .collect()
    })
}

impl BlockState {
    pub const AIR: BlockState = BlockState(0);
    pub fn block(self) -> &'static BlockInfo {
        &BLOCKS[STATE_BLOCK[self.0 as usize] as usize]
    }
    pub fn block_index(self) -> usize {
        STATE_BLOCK[self.0 as usize] as usize
    }
    pub fn name(self) -> &'static str {
        self.block().name
    }
    pub fn is_air(self) -> bool {
        matches!(self.name(), "air" | "cave_air" | "void_air")
    }
    pub fn is_valid(id: u32) -> bool {
        (id as usize) < STATE_COUNT
    }
    /// Default state of a block by name ("stone" or "minecraft:stone").
    pub fn from_name(name: &str) -> Option<BlockState> {
        let n = name.trim_start_matches("minecraft:");
        name_index()
            .get(n)
            .map(|i| BlockState(BLOCKS[*i as usize].default_state))
    }
    /// Parse "oak_log[axis=x]" style strings.
    pub fn parse(s: &str) -> Option<BlockState> {
        let (n, props) = match s.find('[') {
            Some(i) => (&s[..i], s[i + 1..].trim_end_matches(']')),
            None => (s, ""),
        };
        let mut st = Self::from_name(n)?;
        for kv in props.split(',').filter(|x| !x.is_empty()) {
            let (k, v) = kv.split_once('=')?;
            st = st.with(k.trim(), v.trim())?;
        }
        Some(st)
    }
    pub fn values(self) -> &'static [u8] {
        let a = STATE_VAL_OFF[self.0 as usize] as usize;
        let b = STATE_VAL_OFF[self.0 as usize + 1] as usize;
        &STATE_VALS[a..b]
    }
    pub fn get(self, prop: &str) -> Option<&'static str> {
        let b = self.block();
        let i = b.props.iter().position(|(k, _)| *k == prop)?;
        Some(b.props[i].1[self.values()[i] as usize])
    }
    pub fn props(self) -> Vec<(&'static str, &'static str)> {
        let b = self.block();
        let v = self.values();
        b.props
            .iter()
            .enumerate()
            .map(|(i, (k, vs))| (*k, vs[v[i] as usize]))
            .collect()
    }
    /// Return the state with `prop` set to `value`, searching the block's states.
    pub fn with(self, prop: &str, value: &str) -> Option<BlockState> {
        let b = self.block();
        let pi = b.props.iter().position(|(k, _)| *k == prop)?;
        let vi = b.props[pi].1.iter().position(|v| *v == value)? as u8;
        let mut want = self.values().to_vec();
        want[pi] = vi;
        (b.first_state..b.first_state + b.num_states)
            .map(BlockState)
            .find(|s| s.values() == &want[..])
    }
    pub fn to_string(self) -> String {
        let p = self.props();
        if p.is_empty() {
            format!("minecraft:{}", self.name())
        } else {
            format!(
                "minecraft:{}[{}]",
                self.name(),
                p.iter()
                    .map(|(k, v)| format!("{k}={v}"))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}
