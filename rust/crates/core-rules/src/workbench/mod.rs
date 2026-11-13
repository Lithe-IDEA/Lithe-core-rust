//! Workbench chrome rules that a host runs when the user rearranges the window.

mod chrome_layout;

pub use chrome_layout::{
    reduce as reduce_chrome_layout, reduce_json as reduce_chrome_layout_json, Action, CatalogItem,
    Placement, Reduction, State, Surface, Zone, REDUCE_CHROME_LAYOUT,
};
