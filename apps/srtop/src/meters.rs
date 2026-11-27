//! Meter layout and non-graph display modes (PX-070; design §§7.4, 10–12;
//! D2 T16, T28).
//!
//! The system summary's figures (PX-007) are *meters*: overall CPU, memory,
//! swap, load average, uptime and the process count. Which of them are shown,
//! in which order, in which of [`MAX_METER_GROUPS`] semantic groups, whether
//! the meters are shown at all, and whether a bar-capable meter shows a bar
//! beside its line or its line alone, is a [`MeterLayout`]: server-owned
//! application state (§6.3) that a saved view (PX-069) stores.
//!
//! A layout is semantic, never geometric (§10). A group is a standard `Column`
//! inside the summary's `Row` of groups: the client decides where it goes, how
//! wide it is, how it wraps at a narrow width and how it looks in any theme.
//! No position, size, window dimension or colour is ever sent, so a native
//! resize or appearance change needs nothing from the server.
//!
//! Structure follows the layout and nothing else (§12). Every summary node
//! exists in every layout, because a node ID is never reused in a session
//! (§6.2): a meter that is not shown, and the bar of a meter shown as text,
//! sit in a collapsed container ([`summary::METER_HIDDEN`]), and hiding every
//! meter collapses the row of groups. Changing the layout is one transaction:
//! a `MOVE_NODE` for each node that changes place, plus the group visibility,
//! the control labels and the figures of a meter that is shown again; it
//! carries no figure a client already shows, and an unchanged layout commits
//! nothing. A sample sets only the properties that changed
//! (`SET_PROPERTY`, `CLEAR_PROPERTY`), and only on nodes that are shown: a
//! hidden meter, a bar shown as text and every meter while the meters are
//! hidden keep what the client holds until they are shown again.
//!
//! Every meter has a text presentation, and a bar is never shown without its
//! line: every displayed value has a numeric or textual fallback, and a value
//! that is unavailable, denied, warming up or stale is worded on its line and
//! never drawn as an empty bar (PX-007).
//!
//! The CPU meter is one aggregate over every logical CPU the source lists: its
//! node count does not depend on how many there are. Per-logical-CPU meters
//! (PX-052) are a collector this build does not have; when they arrive they
//! are a collection model, not a node per CPU.
use crate::metric;
use crate::summary::{
    self, Properties, CPU_BAR, CPU_ROW, CPU_TEXT, FRESHNESS_TEXT, LOAD_TEXT, MEMORY_BAR,
    MEMORY_ROW, MEMORY_TEXT, METER_GROUPS, METER_GROUP_1, METER_GROUP_2, METER_HIDDEN,
    PROCESSES_TEXT, SUMMARY, SWAP_BAR, SWAP_ROW, SWAP_TEXT, UPTIME_TEXT,
};
use srui_sdk::{
    EnumToken, NodeId, Operation, PropertyRef, SpacingRole, TextRole, TypeRef, Value, Visibility,
};
use std::collections::BTreeSet;

/// How many semantic groups the meters can be placed in: two, like htop's
/// left and right header columns. A bound, so the structure a layout can ask
/// for is a constant.
pub const MAX_METER_GROUPS: usize = 2;

/// The group `Column` nodes, in group order.
pub const GROUP_NODES: [NodeId; MAX_METER_GROUPS] = [METER_GROUP_1, METER_GROUP_2];

/// The longest meter layout status, in encoded bytes.
pub const MAX_METERS_STATUS_BYTES: usize = 256;

/// One meter family the summary can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Meter {
    Cpu,
    Memory,
    Swap,
    Load,
    Uptime,
    Processes,
}

/// Every meter, in its default order.
pub const METERS: [Meter; 6] = [
    Meter::Cpu,
    Meter::Memory,
    Meter::Swap,
    Meter::Load,
    Meter::Uptime,
    Meter::Processes,
];

impl Meter {
    /// The meter's key in a settings file: its metric's identifier, which
    /// names the same figure in every build that has it.
    pub fn key(self) -> &'static str {
        match self {
            Self::Cpu => metric::SYSTEM_CPU.id,
            Self::Memory => metric::MEMORY_USED.id,
            Self::Swap => metric::SWAP_USED.id,
            Self::Load => metric::LOAD_AVERAGE.id,
            Self::Uptime => metric::UPTIME.id,
            Self::Processes => metric::PROCESS_COUNT.id,
        }
    }

    /// The meter named by `key`, if this build has it.
    pub fn of_key(key: &str) -> Option<Self> {
        METERS.into_iter().find(|meter| meter.key() == key)
    }

    /// The short name its controls and the layout status use.
    pub fn name(self) -> &'static str {
        match self {
            Self::Cpu => "CPU",
            Self::Memory => "Memory",
            Self::Swap => "Swap",
            Self::Load => "Load",
            Self::Uptime => "Uptime",
            Self::Processes => "Processes",
        }
    }

    /// Whether the meter has a share a bar can show: CPU, memory and swap.
    /// The others are text only.
    pub fn supports_bar(self) -> bool {
        self.bar_parts().is_some()
    }

    /// The node a group holds for this meter.
    pub fn root(self) -> NodeId {
        match self {
            Self::Cpu => CPU_ROW,
            Self::Memory => MEMORY_ROW,
            Self::Swap => SWAP_ROW,
            Self::Load => LOAD_TEXT,
            Self::Uptime => UPTIME_TEXT,
            Self::Processes => PROCESSES_TEXT,
        }
    }

    /// A bar-capable meter's bar and line, inside its row.
    fn bar_parts(self) -> Option<(NodeId, NodeId)> {
        match self {
            Self::Cpu => Some((CPU_BAR, CPU_TEXT)),
            Self::Memory => Some((MEMORY_BAR, MEMORY_TEXT)),
            Self::Swap => Some((SWAP_BAR, SWAP_TEXT)),
            Self::Load | Self::Uptime | Self::Processes => None,
        }
    }

    /// The presentation a newly shown meter starts in.
    pub fn default_presentation(self) -> Presentation {
        if self.supports_bar() {
            Presentation::Bar
        } else {
            Presentation::Text
        }
    }
}

/// How one meter is shown. Both are standard widgets: a `Progress` bar with
/// its line, or the line alone. A graph is a later negotiated extension
/// (PX-071–PX-073), not a mode here.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Presentation {
    /// The bar, then the line. Only a meter that [`Meter::supports_bar`].
    Bar,
    /// The line alone. Every meter supports it.
    Text,
}

impl Presentation {
    pub fn key(self) -> &'static str {
        match self {
            Self::Bar => "bar",
            Self::Text => "text",
        }
    }

    pub fn of_key(key: &str) -> Option<Self> {
        match key {
            "bar" => Some(Self::Bar),
            "text" => Some(Self::Text),
            _ => None,
        }
    }
}

/// One shown meter and how it is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Placement {
    pub meter: Meter,
    pub presentation: Presentation,
}

/// Why a set of groups is not a layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    /// A meter placed twice.
    Duplicate(Meter),
    /// A bar asked of a text-only meter.
    Unsupported(Meter),
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Duplicate(meter) => {
                write!(formatter, "the {} meter is placed twice", meter.name())
            }
            Self::Unsupported(meter) => {
                write!(formatter, "the {} meter has no bar", meter.name())
            }
        }
    }
}

impl std::error::Error for LayoutError {}

/// Which meters are shown, in which group and order, and how; and whether
/// the meters are shown at all.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct MeterLayout {
    header: bool,
    groups: [Vec<Placement>; MAX_METER_GROUPS],
}

impl Default for MeterLayout {
    /// Every meter in the first group, in [`METERS`] order, each in its
    /// default presentation; the second group empty; the meters shown. This
    /// is the summary PX-007 published.
    fn default() -> Self {
        Self {
            header: true,
            groups: [
                METERS
                    .into_iter()
                    .map(|meter| Placement {
                        meter,
                        presentation: meter.default_presentation(),
                    })
                    .collect(),
                Vec::new(),
            ],
        }
    }
}

impl MeterLayout {
    /// A layout of `groups`, the meters shown when `header` is on. Each meter
    /// is placed at most once, and only a bar-capable meter as a bar.
    pub fn new(
        header: bool,
        groups: [Vec<Placement>; MAX_METER_GROUPS],
    ) -> Result<Self, LayoutError> {
        let mut seen = BTreeSet::new();
        for placement in groups.iter().flatten() {
            if !seen.insert(placement.meter) {
                return Err(LayoutError::Duplicate(placement.meter));
            }
            if placement.presentation == Presentation::Bar && !placement.meter.supports_bar() {
                return Err(LayoutError::Unsupported(placement.meter));
            }
        }
        Ok(Self { header, groups })
    }

    /// Whether the meters are shown at all.
    pub fn header(&self) -> bool {
        self.header
    }

    pub fn groups(&self) -> &[Vec<Placement>; MAX_METER_GROUPS] {
        &self.groups
    }

    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    /// Where `meter` is placed: its group and its index in it.
    pub fn position(&self, meter: Meter) -> Option<(usize, usize)> {
        self.groups.iter().enumerate().find_map(|(group, placed)| {
            placed
                .iter()
                .position(|placement| placement.meter == meter)
                .map(|index| (group, index))
        })
    }

    /// How `meter` is shown, or `None` when it is not placed.
    pub fn presentation(&self, meter: Meter) -> Option<Presentation> {
        self.position(meter)
            .map(|(group, index)| self.groups[group][index].presentation)
    }

    pub fn shows(&self, meter: Meter) -> bool {
        self.position(meter).is_some()
    }

    /// Whether `meter` can move one place earlier: it is placed, and not
    /// first in the first group.
    pub fn can_move_earlier(&self, meter: Meter) -> bool {
        self.position(meter).is_some_and(|place| place != (0, 0))
    }

    /// Whether `meter` can move one place later: it is placed, and not last
    /// in the last group.
    pub fn can_move_later(&self, meter: Meter) -> bool {
        self.position(meter).is_some_and(|(group, index)| {
            group + 1 < MAX_METER_GROUPS || index + 1 < self.groups[group].len()
        })
    }

    /// `meter` hidden when it is placed, or shown at the end of the first
    /// group in its default presentation when it is not.
    pub fn toggling(&self, meter: Meter) -> Self {
        let mut next = self.clone();
        match self.position(meter) {
            Some((group, index)) => {
                next.groups[group].remove(index);
            }
            None => next.groups[0].push(Placement {
                meter,
                presentation: meter.default_presentation(),
            }),
        }
        next
    }

    /// `meter` one place earlier in its group, or, first in a group, last in
    /// the group before.
    pub fn moving_earlier(&self, meter: Meter) -> Self {
        let mut next = self.clone();
        match self.position(meter) {
            Some((group, 0)) if group > 0 => {
                let placement = next.groups[group].remove(0);
                next.groups[group - 1].push(placement);
            }
            Some((group, index)) if index > 0 => next.groups[group].swap(index, index - 1),
            _ => {}
        }
        next
    }

    /// `meter` one place later in its group, or, last in a group, first in
    /// the group after.
    pub fn moving_later(&self, meter: Meter) -> Self {
        let mut next = self.clone();
        match self.position(meter) {
            Some((group, index)) if index + 1 < self.groups[group].len() => {
                next.groups[group].swap(index, index + 1);
            }
            Some((group, index)) if group + 1 < MAX_METER_GROUPS => {
                let placement = next.groups[group].remove(index);
                next.groups[group + 1].insert(0, placement);
            }
            _ => {}
        }
        next
    }

    /// A bar-capable, placed `meter` switched between bar and text.
    pub fn switching(&self, meter: Meter) -> Self {
        let mut next = self.clone();
        if let (Some((group, index)), true) = (self.position(meter), meter.supports_bar()) {
            let placement = &mut next.groups[group][index];
            placement.presentation = match placement.presentation {
                Presentation::Bar => Presentation::Text,
                Presentation::Text => Presentation::Bar,
            };
        }
        next
    }

    /// The meters hidden when shown, or shown when hidden; the groups kept.
    pub fn toggling_header(&self) -> Self {
        Self {
            header: !self.header,
            groups: self.groups.clone(),
        }
    }

    /// Each summary container's children, in order, as this layout places
    /// them: the two groups' shown meters; each bar-capable meter's row, its
    /// bar first unless it is shown as text; and the collapsed
    /// [`METER_HIDDEN`] container, which holds every meter that is not
    /// shown and every bar of a meter shown as text, in [`METERS`] order.
    /// Every summary node is placed in exactly one container.
    pub fn arrangement(&self) -> Vec<(NodeId, Vec<NodeId>)> {
        let mut arrangement: Vec<(NodeId, Vec<NodeId>)> = GROUP_NODES
            .into_iter()
            .zip(&self.groups)
            .map(|(node, placed)| {
                (
                    node,
                    placed
                        .iter()
                        .map(|placement| placement.meter.root())
                        .collect(),
                )
            })
            .collect();
        let mut hidden: Vec<NodeId> = METERS
            .into_iter()
            .filter(|meter| !self.shows(*meter))
            .map(Meter::root)
            .collect();
        for meter in METERS {
            if let Some((bar, text)) = meter.bar_parts() {
                // A hidden meter's row keeps its bar: the row is out of
                // sight whole, and a meter is shown again as a bar.
                if self.presentation(meter) == Some(Presentation::Text) {
                    arrangement.push((meter.root(), vec![text]));
                    hidden.push(bar);
                } else {
                    arrangement.push((meter.root(), vec![bar, text]));
                }
            }
        }
        arrangement.push((METER_HIDDEN, hidden));
        arrangement
    }

    /// Every summary node with its type and its parent in this layout, in
    /// creation order: the summary column, its row of groups and the group
    /// columns, each group's meters, the freshness line, and the collapsed
    /// container of hidden meters and bars. Every layout has the same nodes
    /// — only their parents and order differ — so no node is ever created
    /// or deleted after the shell is (§6.2: an ID is never reused).
    pub fn nodes(&self) -> Vec<(NodeId, TypeRef, NodeId)> {
        let arrangement = self.arrangement();
        let children = |parent: NodeId| {
            arrangement
                .iter()
                .find(|(node, _)| *node == parent)
                .map(|(_, children)| children.clone())
                .unwrap_or_default()
        };
        let mut nodes = vec![
            (SUMMARY, TypeRef::COLUMN, crate::COLUMN),
            (METER_GROUPS, TypeRef::ROW, SUMMARY),
            (METER_GROUP_1, TypeRef::COLUMN, METER_GROUPS),
            (METER_GROUP_2, TypeRef::COLUMN, METER_GROUPS),
        ];
        // A container's children, each followed by its own children, so a
        // parent is always created before what it holds.
        fn place(
            nodes: &mut Vec<(NodeId, TypeRef, NodeId)>,
            parent: NodeId,
            children: &dyn Fn(NodeId) -> Vec<NodeId>,
        ) {
            for child in children(parent) {
                nodes.push((child, node_type(child), parent));
                place(nodes, child, children);
            }
        }
        for group in GROUP_NODES {
            place(&mut nodes, group, &children);
        }
        nodes.push((FRESHNESS_TEXT, TypeRef::TEXT, SUMMARY));
        nodes.push((METER_HIDDEN, TypeRef::COLUMN, SUMMARY));
        place(&mut nodes, METER_HIDDEN, &children);
        nodes
    }

    /// Whether a sample updates `node` in this layout: a meter's nodes only
    /// while it is shown (and a bar only while its meter is shown as a bar)
    /// and the meters are; every other summary node always. A node that is
    /// not updated keeps what the client holds until it is shown again.
    pub fn updates(&self, node: NodeId) -> bool {
        for meter in METERS {
            let bar = meter.bar_parts();
            if bar.is_some_and(|(bar, _)| bar == node) {
                return self.header && self.presentation(meter) == Some(Presentation::Bar);
            }
            if meter.root() == node || bar.is_some_and(|(_, text)| text == node) {
                return self.header && self.shows(meter);
            }
        }
        true
    }

    /// The summary properties a client is sent in this layout. `full` is
    /// every property the latest sample states, for every node, and
    /// `published` what the client holds: a node this layout
    /// [`updates`](Self::updates) takes `full`'s, any other keeps
    /// `published`'s (or `full`'s, before it was ever published). The
    /// layout's own properties are added: the row of groups is collapsed
    /// while the meters are hidden, a group with no meter is collapsed, and
    /// the container of hidden meters always is.
    pub fn properties(&self, full: &Properties, published: &Properties) -> Properties {
        let mut properties = Properties::new();
        for (node, ..) in summary::LAYOUT {
            let held = || published.keys().any(|(owner, _)| *owner == node);
            let source = if self.updates(node) || !held() {
                full
            } else {
                published
            };
            properties.extend(
                source
                    .iter()
                    .filter(|((owner, _), _)| *owner == node)
                    .map(|(key, value)| (*key, value.clone())),
            );
        }
        let visibility = |shown: bool| {
            Value::from(EnumToken::from(if shown {
                Visibility::Visible
            } else {
                Visibility::Collapsed
            }))
        };
        properties.insert(
            (METER_GROUPS, PropertyRef::VISIBILITY),
            visibility(self.header),
        );
        for (group, node) in GROUP_NODES.into_iter().enumerate() {
            properties.insert(
                (node, PropertyRef::VISIBILITY),
                visibility(self.header && !self.groups[group].is_empty()),
            );
        }
        // Never shown, in any layout.
        properties.insert((METER_HIDDEN, PropertyRef::VISIBILITY), visibility(false));
        properties
    }

    /// What the layout status says: each group's meters and presentation, and
    /// whether the meters are hidden.
    pub fn status(&self) -> String {
        let groups: Vec<String> = self
            .groups
            .iter()
            .enumerate()
            .map(|(group, placed)| {
                let meters = if placed.is_empty() {
                    "empty".to_string()
                } else {
                    placed
                        .iter()
                        .map(|placement| {
                            format!(
                                "{} ({})",
                                placement.meter.name(),
                                placement.presentation.key()
                            )
                        })
                        .collect::<Vec<_>>()
                        .join(", ")
                };
                format!("group {}: {meters}", group + 1)
            })
            .collect();
        let shown = if self.header {
            "Meters shown"
        } else {
            "Meters hidden"
        };
        crate::bounded(
            &format!("{shown} · {}", groups.join(" · ")),
            MAX_METERS_STATUS_BYTES,
        )
    }
}

/// A summary node's widget type, the same in every layout.
fn node_type(node: NodeId) -> TypeRef {
    summary::LAYOUT
        .iter()
        .find(|(id, ..)| *id == node)
        .map_or(TypeRef::TEXT, |(_, node_type, _)| *node_type)
}

/// The operations that create the summary under the shell's column in
/// `layout`, each node carrying its fixed properties and its share of
/// `properties` (already [`MeterLayout::properties`]) — the same single
/// `CREATE_NODE` per node a catch-up snapshot carries (§13, §18).
pub fn create_operations(layout: &MeterLayout, properties: &Properties) -> Vec<Operation> {
    layout
        .nodes()
        .into_iter()
        .map(|(id, node_type, parent)| {
            let mut node = summary::fixed(id);
            node.extend(
                properties
                    .iter()
                    .filter(|((owner, _), _)| *owner == id)
                    .map(|((_, property), value)| (*property, value.clone())),
            );
            Operation::create_node(id, node_type, Some(parent), None, node)
        })
        .collect()
}

/// What changing the published layout `from` into `to` sends, and the
/// summary properties a client then holds.
///
/// `published` is what the client holds now and `full` the latest summary
/// for every node ([`crate::summary::properties`]). The operations are:
///
/// 1. one `MOVE_NODE` for each node that `to` places in another container or
///    at another index than `from` did, computed in container order so that
///    a node already where `to` wants it is never moved;
/// 2. the properties that change: a node `from` and `to` both update keeps
///    exactly what the client holds, so the change carries no figure the
///    client already shows; a node `to` starts updating is brought up to
///    `full`'s figures, because it held frozen ones; and the group
///    visibility follows the layout.
///
/// No node is created or deleted. The same layout yields no operation.
pub fn transition(
    from: &MeterLayout,
    to: &MeterLayout,
    published: &Properties,
    full: &Properties,
) -> (Vec<Operation>, Properties) {
    let mut merged = Properties::new();
    for (node, ..) in summary::LAYOUT {
        let source = if from.updates(node) && to.updates(node) {
            published
        } else {
            full
        };
        merged.extend(
            source
                .iter()
                .filter(|((owner, _), _)| *owner == node)
                .map(|(key, value)| (*key, value.clone())),
        );
    }
    let next = to.properties(&merged, published);
    let mut operations = Vec::new();
    if from == to {
        return (operations, next);
    }
    let mut held = from.arrangement();
    for (container, target) in to.arrangement() {
        let at = held
            .iter()
            .position(|(node, _)| *node == container)
            .expect("every layout has the same containers");
        for (index, child) in target.iter().enumerate() {
            // Where `child` belongs: right after the previous wanted child,
            // whatever not-yet-moved node that leaves still sits between, so
            // a node that is already in order behind one that leaves (a
            // row's line behind the bar going to the hidden container) is
            // never moved.
            let start = |children: &[NodeId]| {
                index.checked_sub(1).map_or(0, |previous| {
                    children
                        .iter()
                        .position(|node| *node == target[previous])
                        .map_or(0, |position| position + 1)
                })
            };
            let from_here = start(&held[at].1);
            let next_staying = held[at].1[from_here..]
                .iter()
                .find(|node| target.contains(node));
            if next_staying == Some(child) {
                continue;
            }
            for (_, children) in held.iter_mut() {
                children.retain(|node| node != child);
            }
            let index = start(&held[at].1);
            held[at].1.insert(index, *child);
            operations.push(Operation::move_node(*child, Some(container), Some(index)));
        }
    }
    for change in summary::changes(published, &next) {
        operations.push(match change.value {
            Some(value) => Operation::set_property(change.node, change.property, value),
            None => Operation::clear_property(change.node, change.property),
        });
    }
    (operations, next)
}

/// What one meter layout button does. Each is resolved against the layout
/// the server holds when the click is applied, never against anything the
/// client supplied.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MeterAction {
    /// Hide the meter when it is shown, show it when it is hidden.
    Toggle(Meter),
    /// One place earlier, or into the end of the previous group.
    Earlier(Meter),
    /// One place later, or into the start of the next group.
    Later(Meter),
    /// Bar or text, for a bar-capable meter.
    Presentation(Meter),
    /// Hide or show every meter.
    Header,
    /// The default layout.
    RestoreDefaults,
}

impl MeterAction {
    /// The layout this action turns `layout` into.
    pub fn apply(self, layout: &MeterLayout) -> MeterLayout {
        match self {
            Self::Toggle(meter) => layout.toggling(meter),
            Self::Earlier(meter) => layout.moving_earlier(meter),
            Self::Later(meter) => layout.moving_later(meter),
            Self::Presentation(meter) => layout.switching(meter),
            Self::Header => layout.toggling_header(),
            Self::RestoreDefaults => MeterLayout::default(),
        }
    }

    /// The button's label under `layout`: what a click would do.
    pub fn label(self, layout: &MeterLayout) -> String {
        match self {
            Self::Toggle(meter) if layout.shows(meter) => format!("Hide {} meter", meter.name()),
            Self::Toggle(meter) => format!("Show {} meter", meter.name()),
            Self::Earlier(meter) => format!("Move {} meter earlier", meter.name()),
            Self::Later(meter) => format!("Move {} meter later", meter.name()),
            Self::Presentation(meter) => match layout.presentation(meter) {
                Some(Presentation::Bar) => format!("Show {} meter as text", meter.name()),
                _ => format!("Show {} meter as bar", meter.name()),
            },
            Self::Header if layout.header() => "Hide meters".to_string(),
            Self::Header => "Show meters".to_string(),
            Self::RestoreDefaults => "Restore default meters".to_string(),
        }
    }

    /// Whether a click would change `layout`: a control that cannot is
    /// disabled, so event validation refuses it (§7.4).
    pub fn enabled(self, layout: &MeterLayout) -> bool {
        self.apply(layout) != *layout
    }

    /// What the button says to assistive technology beyond its label.
    fn description(self) -> Option<&'static str> {
        match self {
            Self::Earlier(_) => Some(
                "Moves the meter one place earlier in its group; the first meter of the second group moves to the end of the first.",
            ),
            Self::Later(_) => Some(
                "Moves the meter one place later in its group; the last meter of the first group moves to the start of the second.",
            ),
            _ => None,
        }
    }
}

/// Every meter layout button with what it does, in published order.
pub const METER_BUTTONS: [(NodeId, MeterAction); 23] = {
    use crate::*;
    use Meter::*;
    use MeterAction::*;
    [
        (METER_TOGGLE_CPU, Toggle(Cpu)),
        (METER_EARLIER_CPU, Earlier(Cpu)),
        (METER_LATER_CPU, Later(Cpu)),
        (METER_MODE_CPU, Presentation(Cpu)),
        (METER_TOGGLE_MEMORY, Toggle(Memory)),
        (METER_EARLIER_MEMORY, Earlier(Memory)),
        (METER_LATER_MEMORY, Later(Memory)),
        (METER_MODE_MEMORY, Presentation(Memory)),
        (METER_TOGGLE_SWAP, Toggle(Swap)),
        (METER_EARLIER_SWAP, Earlier(Swap)),
        (METER_LATER_SWAP, Later(Swap)),
        (METER_MODE_SWAP, Presentation(Swap)),
        (METER_TOGGLE_LOAD, Toggle(Load)),
        (METER_EARLIER_LOAD, Earlier(Load)),
        (METER_LATER_LOAD, Later(Load)),
        (METER_TOGGLE_UPTIME, Toggle(Uptime)),
        (METER_EARLIER_UPTIME, Earlier(Uptime)),
        (METER_LATER_UPTIME, Later(Uptime)),
        (METER_TOGGLE_PROCESSES, Toggle(Processes)),
        (METER_EARLIER_PROCESSES, Earlier(Processes)),
        (METER_LATER_PROCESSES, Later(Processes)),
        (METERS_HEADER, Header),
        (METERS_RESTORE, RestoreDefaults),
    ]
};

/// The operations that create the meter layout controls under the shell's
/// column: a `Row` of the [`METER_BUTTONS`] and the layout status. A shell
/// that collects nothing has them disabled, like its sort buttons.
pub fn create_control_operations(layout: &MeterLayout, enabled: bool) -> Vec<Operation> {
    let mut operations = vec![Operation::create_node(
        crate::METER_CONTROLS_ROW,
        TypeRef::ROW,
        Some(crate::COLUMN),
        None,
        [
            (
                PropertyRef::SPACING_ROLE,
                Value::from(EnumToken::from(SpacingRole::Normal)),
            ),
            (PropertyRef::LABEL, Value::from("Meter layout")),
        ],
    )];
    for (node, action) in METER_BUTTONS {
        let mut properties = vec![
            (PropertyRef::LABEL, Value::String(action.label(layout))),
            (
                PropertyRef::ENABLED,
                Value::Bool(enabled && action.enabled(layout)),
            ),
        ];
        if let Some(description) = action.description() {
            properties.push((
                PropertyRef::ACCESSIBLE_DESCRIPTION,
                Value::from(description),
            ));
        }
        operations.push(Operation::create_node(
            node,
            TypeRef::BUTTON,
            Some(crate::METER_CONTROLS_ROW),
            None,
            properties,
        ));
    }
    operations.push(Operation::create_node(
        crate::METERS_STATUS,
        TypeRef::TEXT,
        Some(crate::METER_CONTROLS_ROW),
        None,
        [
            (PropertyRef::TEXT, Value::String(layout.status())),
            (
                PropertyRef::ROLE,
                Value::from(EnumToken::from(if enabled {
                    TextRole::Status
                } else {
                    TextRole::Caption
                })),
            ),
        ],
    ));
    operations
}

/// The control properties that change between `from` and `to` on a
/// collecting view: each button's label and enablement, and the status.
pub fn control_operations(from: &MeterLayout, to: &MeterLayout) -> Vec<Operation> {
    let mut operations = Vec::new();
    for (node, action) in METER_BUTTONS {
        let label = action.label(to);
        if label != action.label(from) {
            operations.push(Operation::set_property(node, PropertyRef::LABEL, label));
        }
        let enabled = action.enabled(to);
        if enabled != action.enabled(from) {
            operations.push(Operation::set_property(node, PropertyRef::ENABLED, enabled));
        }
    }
    let status = to.status();
    if status != from.status() {
        operations.push(Operation::set_property(
            crate::METERS_STATUS,
            PropertyRef::TEXT,
            status,
        ));
    }
    operations
}

#[cfg(test)]
mod tests {
    use super::*;

    fn placement(meter: Meter, presentation: Presentation) -> Placement {
        Placement {
            meter,
            presentation,
        }
    }

    #[test]
    fn the_default_layout_is_the_pre_px070_summary_in_one_group() {
        let layout = MeterLayout::default();
        assert!(layout.header());
        assert_eq!(layout.groups()[0].len(), METERS.len());
        assert!(layout.groups()[1].is_empty());
        assert_eq!(
            layout.nodes(),
            summary::LAYOUT.to_vec(),
            "summary::LAYOUT is the default layout's nodes"
        );
    }

    #[test]
    fn layouts_refuse_duplicates_and_bars_on_text_meters() {
        assert_eq!(
            MeterLayout::new(
                true,
                [
                    vec![placement(Meter::Cpu, Presentation::Bar)],
                    vec![placement(Meter::Cpu, Presentation::Text)]
                ]
            ),
            Err(LayoutError::Duplicate(Meter::Cpu))
        );
        assert_eq!(
            MeterLayout::new(
                true,
                [vec![placement(Meter::Load, Presentation::Bar)], vec![]]
            ),
            Err(LayoutError::Unsupported(Meter::Load))
        );
    }

    #[test]
    fn moves_cross_the_group_boundary_and_stop_at_the_ends() {
        let layout = MeterLayout::default();
        assert!(!layout.can_move_earlier(Meter::Cpu));
        assert!(layout.can_move_later(Meter::Processes));
        let moved = layout.moving_later(Meter::Processes);
        assert_eq!(moved.position(Meter::Processes), Some((1, 0)));
        assert!(!moved.can_move_later(Meter::Processes));
        assert_eq!(moved.moving_later(Meter::Processes), moved);
        assert_eq!(moved.moving_earlier(Meter::Processes), layout);
        let swapped = layout.moving_earlier(Meter::Memory);
        assert_eq!(swapped.position(Meter::Memory), Some((0, 0)));
        assert_eq!(swapped.position(Meter::Cpu), Some((0, 1)));
        assert_eq!(
            layout
                .toggling(Meter::Swap)
                .toggling(Meter::Swap)
                .position(Meter::Swap),
            Some((0, 5))
        );
        assert_eq!(
            layout.switching(Meter::Load),
            layout,
            "a text meter has no bar"
        );
        assert!(!MeterAction::Presentation(Meter::Load).enabled(&layout));
        assert!(!MeterAction::RestoreDefaults.enabled(&layout));
        let hidden = layout.toggling(Meter::Cpu);
        for action in [
            MeterAction::Earlier(Meter::Cpu),
            MeterAction::Later(Meter::Cpu),
            MeterAction::Presentation(Meter::Cpu),
        ] {
            assert!(!action.enabled(&hidden), "{action:?} on a hidden meter");
        }
    }

    /// Every reachable layout is bounded: at most every meter once, so the
    /// summary never holds more than its default node count plus nothing.
    #[test]
    fn every_layout_reachable_by_the_controls_stays_within_the_summary_nodes() {
        let mut frontier = vec![MeterLayout::default()];
        let mut seen = std::collections::HashSet::new();
        while let Some(layout) = frontier.pop() {
            if !seen.insert(layout.clone()) || seen.len() > 50_000 {
                continue;
            }
            let nodes = layout.nodes();
            assert_eq!(nodes.len(), summary::NODE_COUNT);
            let mut ids: Vec<NodeId> = nodes.iter().map(|(id, ..)| *id).collect();
            ids.sort();
            ids.dedup();
            assert_eq!(ids.len(), summary::NODE_COUNT, "every node placed once");
            assert!(layout.status().len() <= MAX_METERS_STATUS_BYTES);
            for (_, action) in METER_BUTTONS {
                frontier.push(action.apply(&layout));
            }
        }
        assert!(
            seen.len() > 1_000,
            "the walk explored {} layouts",
            seen.len()
        );
    }

    /// Applies `operations`' moves to `arrangement` with the store's own
    /// semantics: remove, then insert at the index, clamped.
    fn moved(
        mut arrangement: Vec<(NodeId, Vec<NodeId>)>,
        operations: &[Operation],
    ) -> Vec<(NodeId, Vec<NodeId>)> {
        for operation in operations {
            if let Operation::MoveNode {
                id,
                new_parent_id: Some(parent),
                new_child_index: Some(index),
            } = operation
            {
                let length = arrangement
                    .iter()
                    .find(|(node, _)| node == parent)
                    .unwrap()
                    .1
                    .len();
                assert!(*index <= length, "{operation:?} is out of bounds");
                for (_, children) in arrangement.iter_mut() {
                    children.retain(|node| node != id);
                }
                let children = &mut arrangement
                    .iter_mut()
                    .find(|(node, _)| node == parent)
                    .unwrap()
                    .1;
                children.insert((*index).min(children.len()), *id);
            }
        }
        arrangement
    }

    /// Between any two layouts the controls reach, the moves turn one
    /// arrangement into the other exactly, and one click moves one node (two
    /// to hide a meter shown as text).
    #[test]
    fn the_moves_between_any_two_layouts_reach_the_target_and_a_click_moves_one_node() {
        let full = summary::properties(&summary::summarize(&summary::State::NotStarted));
        let mut layouts = vec![MeterLayout::default()];
        let mut index = 0;
        while index < layouts.len() && layouts.len() < 400 {
            for (_, action) in METER_BUTTONS {
                let next = action.apply(&layouts[index]);
                if !layouts.contains(&next) {
                    layouts.push(next);
                }
            }
            index += 1;
        }
        for from in &layouts {
            let published = from.properties(&full, &Properties::new());
            for (_, action) in METER_BUTTONS {
                let to = action.apply(from);
                let (operations, _) = transition(from, &to, &published, &full);
                let moves = operations
                    .iter()
                    .filter(|op| matches!(op, Operation::MoveNode { .. }))
                    .count();
                // One node, or two when a meter shown as text is hidden:
                // its row, and its bar back into the row it is shown in
                // next.
                let hiding_text = matches!(action, MeterAction::Toggle(meter)
                    if from.presentation(meter) == Some(Presentation::Text)
                        && meter.supports_bar());
                // Restoring the defaults is a jump: as many as differ.
                let most = match action {
                    MeterAction::RestoreDefaults => summary::NODE_COUNT,
                    _ if hiding_text => 2,
                    _ => 1,
                };
                assert!(moves <= most, "{action:?} on {from:?}: {operations:?}");
                assert_eq!(moves > 0, from.arrangement() != to.arrangement());
            }
            for to in layouts.iter().step_by(7) {
                let (operations, _) = transition(from, to, &published, &full);
                assert_eq!(moved(from.arrangement(), &operations), to.arrangement());
                assert!(operations.iter().all(|op| !matches!(
                    op,
                    Operation::CreateNode { .. } | Operation::DeleteNode { .. }
                )));
            }
        }
    }

    #[test]
    fn the_same_layout_sends_nothing_and_a_change_sends_no_figure() {
        let full = summary::properties(&summary::summarize(&summary::State::NotStarted));
        let layout = MeterLayout::default();
        let published = layout.properties(&full, &Properties::new());
        let (operations, next) = transition(&layout, &layout, &published, &full);
        assert!(operations.is_empty());
        assert_eq!(next, published);
        assert!(control_operations(&layout, &layout).is_empty());

        // A figure the client holds is kept even when `full` has moved on.
        let mut moved_on = full.clone();
        moved_on.insert((CPU_TEXT, PropertyRef::TEXT), Value::from("newer"));
        let to = layout.moving_later(Meter::Processes);
        let (operations, next) = transition(&layout, &to, &published, &moved_on);
        assert_eq!(
            next.get(&(CPU_TEXT, PropertyRef::TEXT)),
            published.get(&(CPU_TEXT, PropertyRef::TEXT))
        );
        assert_eq!(
            operations,
            vec![
                Operation::move_node(PROCESSES_TEXT, Some(METER_GROUP_2), Some(0)),
                Operation::set_property(
                    METER_GROUP_2,
                    PropertyRef::VISIBILITY,
                    Value::from(EnumToken::from(Visibility::Visible))
                ),
            ]
        );
        assert_eq!(next, to.properties(&full, &published));
    }
}
