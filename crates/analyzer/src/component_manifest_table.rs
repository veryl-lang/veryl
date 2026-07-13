//! Interface manifests of user-defined components, registered at
//! `Analyzer::new` from previously built sidecar files. Keys match the
//! `TbComponentKind::External` resolution keys (`<name>` /
//! `<project>::<name>`). Best-effort: an absent manifest simply skips the
//! analysis-time interface checks.

use crate::HashMap;
use std::cell::RefCell;
use std::rc::Rc;
use veryl_metadata::ComponentManifest;
use veryl_parser::resource_table::StrId;

thread_local!(
    static TABLE: RefCell<HashMap<StrId, Rc<ComponentManifest>>> =
        RefCell::new(HashMap::default())
);

pub fn insert(key: StrId, manifest: ComponentManifest) {
    TABLE.with(|f| f.borrow_mut().insert(key, Rc::new(manifest)));
}

pub fn get(key: StrId) -> Option<Rc<ComponentManifest>> {
    TABLE.with(|f| f.borrow().get(&key).cloned())
}

pub fn clear() {
    TABLE.with(|f| f.borrow_mut().clear())
}
