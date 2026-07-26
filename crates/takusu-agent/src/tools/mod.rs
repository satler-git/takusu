pub mod day_details;
pub mod memory;
pub mod progress;
pub mod rrule;
pub mod skills;
pub mod takusu;
pub mod tool_search;
pub mod user_input;

use crate::ToolError;

pub(crate) fn other_error(msg: impl Into<String>) -> ToolError {
    ToolError::Other(Box::new(std::io::Error::other(msg.into())))
}
