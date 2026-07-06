//! report's external-tool dependencies, advertised in `clyde report --help`.
//!
//! The render/collect paths shell out to these binaries (not linked libraries), so report's help
//! lists them and whether each is installed. Rendering + probing lives in [`common::tools`]; this
//! module just declares report's list.

use common::{Tool, required_tools_help};

/// The "REQUIRED TOOLS" block for `clyde report`, listing the external binaries report invokes.
pub fn tool_validation_help() -> String {
    required_tools_help(&[
        Tool {
            name: "persona",
            purpose: "report render: persona block in context",
        },
        Tool {
            name: "pandoc",
            purpose: "report render --format pdf",
        },
        Tool {
            name: "marquee",
            purpose: "report render --format marquee-html / marquee-markdown",
        },
        Tool {
            name: "git",
            purpose: "report collect: repo detection",
        },
        Tool {
            name: "jq",
            purpose: "report collect: query JSON report output",
        },
    ])
}

#[cfg(test)]
mod tests;
