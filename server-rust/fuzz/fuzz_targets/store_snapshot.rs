//! Full observable semantic-store snapshot used by mutation fuzz targets.

use srui_semantic_tree::{
    ItemId, ModelId, NodeId, PropertyRef, Range, Revision, SemanticStore, TypeRef, Value,
};
use std::collections::BTreeMap;

type NodeSnapshot = (
    TypeRef,
    Option<NodeId>,
    Vec<NodeId>,
    BTreeMap<PropertyRef, Value>,
);

#[derive(Clone, Debug, PartialEq)]
struct ModelSnapshot {
    model_type: TypeRef,
    item_count: u64,
    cached_item_count: usize,
    cached_ranges: Vec<Range>,
    items_by_index: BTreeMap<u64, (ItemId, Value, BTreeMap<PropertyRef, Value>)>,
}

#[derive(Clone, Debug, PartialEq)]
pub(crate) struct StoreSnapshot {
    revision: Revision,
    node_count: usize,
    roots: Vec<NodeId>,
    nodes: BTreeMap<NodeId, NodeSnapshot>,
    model_count: usize,
    models: BTreeMap<ModelId, ModelSnapshot>,
}

pub(crate) fn take_snapshot(store: &SemanticStore) -> StoreSnapshot {
    let mut nodes = BTreeMap::new();
    for &root in store.root_ids() {
        collect_node(store, root, &mut nodes);
    }
    assert_eq!(
        nodes.len(),
        store.node_count(),
        "every committed node must be reachable from exactly one root"
    );

    let models = store
        .model_ids()
        .filter_map(|model_id| {
            store.get_model(model_id).map(|model| {
                let items_by_index = model
                    .items
                    .iter()
                    .map(|(index, item)| {
                        (
                            *index,
                            (
                                item.item_id,
                                item.value.clone(),
                                item.properties
                                    .iter()
                                    .map(|(property, value)| (*property, value.clone()))
                                    .collect(),
                            ),
                        )
                    })
                    .collect();
                (
                    model_id,
                    ModelSnapshot {
                        model_type: model.model_type,
                        item_count: model.item_count,
                        cached_item_count: model.cached_item_count(),
                        cached_ranges: model.cached_ranges(),
                        items_by_index,
                    },
                )
            })
        })
        .collect();

    StoreSnapshot {
        revision: store.revision(),
        node_count: store.node_count(),
        roots: store.root_ids().to_vec(),
        nodes,
        model_count: store.model_count(),
        models,
    }
}

fn collect_node(
    store: &SemanticStore,
    node_id: NodeId,
    nodes: &mut BTreeMap<NodeId, NodeSnapshot>,
) {
    let node = store
        .get_node(node_id)
        .expect("root and child references must resolve to committed nodes");
    let previous = nodes.insert(
        node_id,
        (
            node.node_type,
            node.parent_id,
            node.ordered_children.clone(),
            node.properties
                .iter()
                .map(|(property, value)| (*property, value.clone()))
                .collect(),
        ),
    );
    assert!(
        previous.is_none(),
        "node graph must be acyclic and singly parented"
    );
    for &child_id in &node.ordered_children {
        collect_node(store, child_id, nodes);
    }
}
