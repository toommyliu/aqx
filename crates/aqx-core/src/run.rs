use std::fmt;

use crate::Halt;

#[derive(Debug)]
pub enum Outcome {
    Finished,
    Stopped(Option<String>),
}

#[derive(Debug)]
pub enum ScriptError {
    Load(String),
    Failed(String),
    Halted(Halt),
}

impl fmt::Display for ScriptError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ScriptError::Load(why) | ScriptError::Failed(why) => f.write_str(why),
            ScriptError::Halted(Halt::Closed(why)) => write!(f, "{why}"),
            ScriptError::Halted(halt) => write!(f, "{halt:?}"),
        }
    }
}
