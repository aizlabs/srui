"""Independent oracle for the cross-runtime sequential state-machine trace.

The Rust and Swift conformance runners consume the same JSON fixture.  This module deliberately
uses neither implementation (nor generated protocol types) to derive the fixture's final state.
"""

from __future__ import annotations

import copy
import json
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any


FIXTURE = (
    Path(__file__).resolve().parent.parent
    / "conformance-vectors"
    / "suites"
    / "01-core-state-machine"
    / "vectors"
    / "13_sequential_transactions.json"
)


@dataclass
class OracleNode:
    node_id: int
    node_type: Any
    parent_id: int | None
    ordered_children: list[int] = field(default_factory=list)
    properties: dict[str, Any] = field(default_factory=dict)


class NodeTraceOracle:
    """Small, intentionally independent interpreter for established node operations."""

    def __init__(self, revision: int) -> None:
        self.revision = revision
        self.roots: list[int] = []
        self.nodes: dict[int, OracleNode] = {}

    def apply(self, transaction: dict[str, Any]) -> None:
        assert transaction["base_revision"] == self.revision
        assert transaction["new_revision"] > self.revision
        staged = copy.deepcopy(self)
        for operation in transaction["operations"]:
            staged._apply_operation(operation)
        staged.revision = transaction["new_revision"]
        self.__dict__ = staged.__dict__

    def _apply_operation(self, operation: dict[str, Any]) -> None:
        operation_type = operation["type"]
        if operation_type == "CREATE_NODE":
            self._create_node(operation)
        elif operation_type == "SET_PROPERTY":
            self.nodes[operation["node_id"]].properties[operation["property"]] = (
                copy.deepcopy(operation["value"])
            )
        elif operation_type == "CLEAR_PROPERTY":
            self.nodes[operation["node_id"]].properties.pop(operation["property"], None)
        elif operation_type == "BATCH_PROPERTY_SET":
            for entry in operation["properties"]:
                self.nodes[operation["node_id"]].properties[entry["property"]] = (
                    copy.deepcopy(entry["value"])
                )
        elif operation_type == "REORDER_CHILDREN":
            parent = self.nodes[operation["parent_id"]]
            new_order = operation["new_order"]
            assert sorted(new_order) == sorted(parent.ordered_children)
            parent.ordered_children = list(new_order)
        elif operation_type == "MOVE_NODE":
            self._move_node(operation)
        elif operation_type == "DELETE_NODE":
            self._delete_node(operation["node_id"])
        else:
            raise AssertionError(
                f"unsupported independent-oracle operation: {operation_type}"
            )

    def _create_node(self, operation: dict[str, Any]) -> None:
        node_id = operation["node_id"]
        assert node_id not in self.nodes
        parent_id = operation["parent_id"]
        node = OracleNode(
            node_id=node_id,
            node_type=operation["node_type"],
            parent_id=parent_id,
            properties=copy.deepcopy(operation.get("properties", {})),
        )
        self.nodes[node_id] = node
        siblings = (
            self.roots if parent_id is None else self.nodes[parent_id].ordered_children
        )
        index = operation.get("child_index")
        siblings.insert(len(siblings) if index is None else index, node_id)

    def _move_node(self, operation: dict[str, Any]) -> None:
        node = self.nodes[operation["node_id"]]
        old_siblings = (
            self.roots
            if node.parent_id is None
            else self.nodes[node.parent_id].ordered_children
        )
        old_siblings.remove(node.node_id)
        node.parent_id = operation["new_parent_id"]
        new_siblings = (
            self.roots
            if node.parent_id is None
            else self.nodes[node.parent_id].ordered_children
        )
        index = operation.get("new_child_index")
        new_siblings.insert(len(new_siblings) if index is None else index, node.node_id)

    def _delete_node(self, node_id: int) -> None:
        node = self.nodes[node_id]
        for child_id in list(node.ordered_children):
            self._delete_node(child_id)
        siblings = (
            self.roots
            if node.parent_id is None
            else self.nodes[node.parent_id].ordered_children
        )
        siblings.remove(node_id)
        del self.nodes[node_id]

    def store_state(self) -> dict[str, Any]:
        return {
            "node_count": len(self.nodes),
            "roots": self.roots,
            "nodes": {
                str(node_id): {
                    "node_id": node.node_id,
                    "node_type": node.node_type,
                    "parent_id": node.parent_id,
                    "ordered_children": node.ordered_children,
                    "properties": node.properties,
                }
                for node_id, node in sorted(self.nodes.items())
            },
            "model_count": 0,
            "models": {},
        }


def test_cross_runtime_sequential_trace_has_independent_expected_state() -> None:
    fixture = json.loads(FIXTURE.read_text(encoding="utf-8"))
    oracle = NodeTraceOracle(fixture.get("initial_revision", 0))

    for transaction in [*fixture["setup_transactions"], fixture["transaction"]]:
        oracle.apply(transaction)

    expected = fixture["expected_outcome"]
    assert expected["status"] == "success"
    assert oracle.revision == expected["committed_revision"]
    assert oracle.store_state() == expected["store_state"]
