//! Accessibility — 无障碍树访问。
//!
//! D 类 method 纯本地状态:
//! - `snapshot() -> Option<AXNode>`(本地缓存)
//! - `has_snapshot() -> bool`
//! - `root_node() -> Option<AXNode>`
//!
//! 非 D 类 method(getFullAXTree)走 transport。
//!
//! @trace REQ-BAO-API-006 [class:Accessibility]

use std::cell::RefCell;
use std::collections::HashMap;

/// AXNode — accessibility tree 节点。
#[derive(Debug, Clone, Default)]
pub struct AXNode {
    pub node_id: String,
    pub role: String,
    pub name: String,
    pub value: Option<serde_json::Value>,
    pub description: String,
    pub properties: HashMap<String, serde_json::Value>,
    pub child_ids: Vec<String>,
    pub ignored: bool,
}

/// Accessibility 本地状态(Page 持有的实例)。
///
/// @trace REQ-BAO-API-006 [class:Accessibility]
pub struct Accessibility {
    /// 缓存的快照(B 类 method 填入)。
    snapshot: RefCell<Option<AXNode>>,
    /// 缓存的根节点 ID。
    root_id: RefCell<Option<String>>,
    /// 所有节点(按 ID 索引)。
    nodes: RefCell<HashMap<String, AXNode>>,
}

impl std::fmt::Debug for Accessibility {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Accessibility")
            .field("has_snapshot", &self.snapshot.borrow().is_some())
            .field("node_count", &self.nodes.borrow().len())
            .finish()
    }
}

impl Accessibility {
    /// 构造 Accessibility(初始无快照)。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn new() -> Self {
        Self {
            snapshot: RefCell::new(None),
            root_id: RefCell::new(None),
            nodes: RefCell::new(HashMap::new()),
        }
    }

    /// 是否有快照。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn has_snapshot(&self) -> bool {
        self.snapshot.borrow().is_some()
    }

    /// 快照(克隆)。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn snapshot(&self) -> Option<AXNode> {
        self.snapshot.borrow().clone()
    }

    /// 设置快照。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn set_snapshot(&self, n: AXNode) {
        let id = n.node_id.clone();
        *self.root_id.borrow_mut() = Some(id);
        *self.snapshot.borrow_mut() = Some(n);
    }

    /// 根节点(从 nodes HashMap)。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn root_node(&self) -> Option<AXNode> {
        let root_id = self.root_id.borrow().clone()?;
        self.nodes.borrow().get(&root_id).cloned()
    }

    /// 按 ID 查找节点。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn node_by_id(&self, id: &str) -> Option<AXNode> {
        self.nodes.borrow().get(id).cloned()
    }

    /// 添加节点(本地缓存)。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn add_node(&self, n: AXNode) {
        self.nodes.borrow_mut().insert(n.node_id.clone(), n);
    }

    /// 节点总数。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn node_count(&self) -> usize {
        self.nodes.borrow().len()
    }

    /// 设置根 ID。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn set_root_id(&self, id: impl Into<String>) {
        *self.root_id.borrow_mut() = Some(id.into());
    }

    /// 重置。
    ///
    /// @trace REQ-BAO-API-006 [class:Accessibility]
    pub fn reset(&self) {
        *self.snapshot.borrow_mut() = None;
        *self.root_id.borrow_mut() = None;
        self.nodes.borrow_mut().clear();
    }
}

impl Default for Accessibility {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_node(id: &str, role: &str) -> AXNode {
        AXNode {
            node_id: id.into(),
            role: role.into(),
            ..Default::default()
        }
    }

    #[test]
    fn new_no_snapshot() {
        let a = Accessibility::new();
        assert!(!a.has_snapshot());
        assert!(a.snapshot().is_none());
        assert_eq!(a.node_count(), 0);
    }

    #[test]
    fn set_snapshot() {
        let a = Accessibility::new();
        let n = make_node("root", "rootWebArea");
        a.set_snapshot(n);
        assert!(a.has_snapshot());
        let s = a.snapshot().unwrap();
        assert_eq!(s.role, "rootWebArea");
    }

    #[test]
    fn add_nodes_and_lookup() {
        let a = Accessibility::new();
        a.add_node(make_node("1", "button"));
        a.add_node(make_node("2", "link"));
        assert_eq!(a.node_count(), 2);
        assert_eq!(a.node_by_id("1").unwrap().role, "button");
        assert!(a.node_by_id("nope").is_none());
    }

    #[test]
    fn root_node_lookup() {
        let a = Accessibility::new();
        a.add_node(make_node("root", "rootWebArea"));
        a.add_node(make_node("btn1", "button"));
        a.set_root_id("root");
        assert_eq!(a.root_node().unwrap().role, "rootWebArea");
    }

    #[test]
    fn reset_clears() {
        let a = Accessibility::new();
        a.set_snapshot(make_node("r", "root"));
        a.add_node(make_node("1", "button"));
        a.reset();
        assert!(!a.has_snapshot());
        assert_eq!(a.node_count(), 0);
    }
}
