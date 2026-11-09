//! Trio encoder: one record per row, `---` between records, each tag on its
//! own line as `name` (Marker), `name:<zinc>`, or, when the Zinc spans lines
//! (a grid, or a List/Dict holding one), `name:Zinc:` plus the indented
//! Zinc. Ours rather than libhaystack's `TrioWriter`, so values go through
//! openhay's Zinc writer (escaped XStr, exact DateTime zones).

use libhaystack::val::{Grid, Value};

pub fn encode_grid(grid: &Grid) -> crate::Result<String> {
    let mut out = String::new();
    for (i, row) in grid.rows.iter().enumerate() {
        if i > 0 {
            out.push_str("---\n");
        }
        for (k, v) in row.iter() {
            match v {
                Value::Null => continue,
                Value::Marker => out.push_str(k),
                other => {
                    let z = match other {
                        Value::Grid(g) => crate::zinc::encode_grid(g)?,
                        v => crate::zinc::encode_value(v)?,
                    };
                    out.push_str(k);
                    out.push(':');
                    if z.contains('\n') {
                        // A grid, or a List/Dict holding one: Trio's
                        // multi-line `Zinc:` block, indented two spaces.
                        out.push_str("Zinc:");
                        for line in z.lines() {
                            out.push_str("\n  ");
                            out.push_str(line);
                        }
                    } else {
                        out.push_str(&z);
                    }
                }
            }
            out.push('\n');
        }
    }
    Ok(out)
}
