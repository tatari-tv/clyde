use comfy_table::{CellAlignment, ContentArrangement, Table};

/// Build a borderless table with the given headers and alignment.
/// Columns default to left-aligned; indices in `right_cols` are right-aligned.
pub fn build(headers: &[&str], rows: Vec<Vec<String>>, right_cols: &[usize]) -> String {
    let mut table = Table::new();
    table
        .load_preset(comfy_table::presets::NOTHING)
        .set_content_arrangement(ContentArrangement::Dynamic);

    table.set_header(headers);

    for row in rows {
        table.add_row(row);
    }

    for (i, col) in table.column_iter_mut().enumerate() {
        if right_cols.contains(&i) {
            col.set_cell_alignment(CellAlignment::Right);
        }
    }

    table.to_string()
}
