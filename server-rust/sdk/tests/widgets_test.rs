//! Comprehensive conformance tests for the SRUI typed widget layer (§7.1–§7.5).
//!
//! Verifies:
//! 1. All 18 required-tier standard widget types (§7.3) constructed via typed API.
//! 2. Documented §7.4 properties round-trip cleanly and match through the generic `SemanticStore` API.
//! 3. Zero divergent state between the typed layer and the underlying `SemanticStore`.
//! 4. `Toggle` implements a single semantic state machine with `value: bool` and `presentation_hint` enum,
//!    and no separate "Checkbox" or "Switch" node types exist.
//! 5. Appearance roles (`TextRole`, `ActionRole`, `InputRole`, `Importance`) are strongly typed.
//! 6. Builder `.into_operation()` works seamlessly in transactional workflows (§12.1).

use srui_sdk::*;
use srui_semantic_tree::{
    EnumToken, ModelId, NodeId, PropertyRef, ResourceHash, SemanticStore, Size, TypeRef, Value,
    STANDARD_NODE_TYPES,
};

#[test]
fn test_all_18_required_tier_node_types_and_constants() {
    assert_eq!(Surface::NODE_TYPE, TypeRef::SURFACE);
    assert_eq!(Surface::NODE_TYPE, TypeRef::standard(1));

    assert_eq!(Row::NODE_TYPE, TypeRef::ROW);
    assert_eq!(Row::NODE_TYPE, TypeRef::standard(3));

    assert_eq!(Column::NODE_TYPE, TypeRef::COLUMN);
    assert_eq!(Column::NODE_TYPE, TypeRef::standard(4));

    assert_eq!(Grid::NODE_TYPE, TypeRef::GRID);
    assert_eq!(Grid::NODE_TYPE, TypeRef::standard(5));

    assert_eq!(Spacer::NODE_TYPE, TypeRef::SPACER);
    assert_eq!(Spacer::NODE_TYPE, TypeRef::standard(6));

    assert_eq!(Separator::NODE_TYPE, TypeRef::SEPARATOR);
    assert_eq!(Separator::NODE_TYPE, TypeRef::standard(7));

    assert_eq!(Scroll::NODE_TYPE, TypeRef::SCROLL);
    assert_eq!(Scroll::NODE_TYPE, TypeRef::standard(8));

    assert_eq!(Text::NODE_TYPE, TypeRef::TEXT);
    assert_eq!(Text::NODE_TYPE, TypeRef::standard(9));

    assert_eq!(RichText::NODE_TYPE, TypeRef::RICHTEXT);
    assert_eq!(RichText::NODE_TYPE, TypeRef::standard(10));

    assert_eq!(Button::NODE_TYPE, TypeRef::BUTTON);
    assert_eq!(Button::NODE_TYPE, TypeRef::standard(11));

    assert_eq!(Toggle::NODE_TYPE, TypeRef::TOGGLE);
    assert_eq!(Toggle::NODE_TYPE, TypeRef::standard(12));

    assert_eq!(TextInput::NODE_TYPE, TypeRef::TEXT_INPUT);
    assert_eq!(TextInput::NODE_TYPE, TypeRef::standard(13));

    assert_eq!(TextArea::NODE_TYPE, TypeRef::TEXT_AREA);
    assert_eq!(TextArea::NODE_TYPE, TypeRef::standard(14));

    assert_eq!(Progress::NODE_TYPE, TypeRef::PROGRESS);
    assert_eq!(Progress::NODE_TYPE, TypeRef::standard(15));

    assert_eq!(Image::NODE_TYPE, TypeRef::IMAGE);
    assert_eq!(Image::NODE_TYPE, TypeRef::standard(16));

    assert_eq!(List::NODE_TYPE, TypeRef::LIST);
    assert_eq!(List::NODE_TYPE, TypeRef::standard(17));

    assert_eq!(Table::NODE_TYPE, TypeRef::TABLE);
    assert_eq!(Table::NODE_TYPE, TypeRef::standard(18));

    assert_eq!(Tree::NODE_TYPE, TypeRef::TREE);
    assert_eq!(Tree::NODE_TYPE, TypeRef::standard(19));
}

#[test]
fn test_no_separate_checkbox_or_switch_node_types_exist() {
    // Confirm across standard registry table that only Toggle (id: 12) exists
    for &(id, name) in STANDARD_NODE_TYPES {
        assert_ne!(name, "Checkbox", "Invariant violated: Checkbox node type found in standard registry");
        assert_ne!(name, "Switch", "Invariant violated: Switch node type found in standard registry");
        if id == 12 {
            assert_eq!(name, "Toggle");
        }
    }
}

#[test]
fn test_surface_widget_roundtrip() {
    let mut store = SemanticStore::new();
    let id = NodeId::new(101);

    let surface = Surface::builder(id)
        .label("Main Window")
        .accessible_description("Primary application window")
        .visibility(Visibility::Visible)
        .enabled(true)
        .busy(false)
        .spacing_role(SpacingRole::Normal)
        .padding_role(PaddingRole::Relaxed)
        .preferred_size(Size::new(800.0, 600.0))
        .minimum_size(Size::new(400.0, 300.0))
        .maximum_size(Size::new(1920.0, 1080.0))
        .horizontal_alignment(HorizontalAlignment::Fill)
        .vertical_alignment(VerticalAlignment::Fill)
        .grow(1.0)
        .shrink(1.0)
        .create(&mut store)
        .expect("Failed to create Surface");

    assert_eq!(surface.id(), id);
    assert_eq!(surface.node_type(), TypeRef::SURFACE);

    // Typed accessors
    assert_eq!(surface.label(&store), Some("Main Window"));
    assert_eq!(surface.accessible_description(&store), Some("Primary application window"));
    assert_eq!(surface.visibility(&store), Some(Visibility::Visible));
    assert_eq!(surface.enabled(&store), Some(true));
    assert!(surface.is_enabled(&store));
    assert_eq!(surface.busy(&store), Some(false));
    assert!(!surface.is_busy(&store));
    assert_eq!(surface.spacing_role(&store), Some(SpacingRole::Normal));
    assert_eq!(surface.padding_role(&store), Some(PaddingRole::Relaxed));
    assert_eq!(surface.preferred_size(&store), Some(Size::new(800.0, 600.0)));
    assert_eq!(surface.minimum_size(&store), Some(Size::new(400.0, 300.0)));
    assert_eq!(surface.maximum_size(&store), Some(Size::new(1920.0, 1080.0)));
    assert_eq!(surface.horizontal_alignment(&store), Some(HorizontalAlignment::Fill));
    assert_eq!(surface.vertical_alignment(&store), Some(VerticalAlignment::Fill));
    assert_eq!(surface.grow(&store), Some(1.0));
    assert_eq!(surface.shrink(&store), Some(1.0));

    // Generic store readback
    {
        let node = store.get_node(id).expect("Node not found in store");
        assert_eq!(node.node_type, TypeRef::SURFACE);
        assert_eq!(node.get_property(PropertyRef::LABEL), Some(&Value::String("Main Window".into())));
        assert_eq!(node.get_property(PropertyRef::ACCESSIBLE_DESCRIPTION), Some(&Value::String("Primary application window".into())));
        assert_eq!(node.get_property(PropertyRef::VISIBILITY), Some(&Value::EnumToken(EnumToken::new(6, 1))));
        assert_eq!(node.get_property(PropertyRef::ENABLED), Some(&Value::Bool(true)));
        assert_eq!(node.get_property(PropertyRef::SPACING_ROLE), Some(&Value::EnumToken(EnumToken::new(7, 3))));
        assert_eq!(node.get_property(PropertyRef::PADDING_ROLE), Some(&Value::EnumToken(EnumToken::new(8, 4))));
        assert_eq!(node.get_property(PropertyRef::PREFERRED_SIZE), Some(&Value::Size(Size::new(800.0, 600.0))));
    }

    // Test setters
    surface.set_label(&mut store, "Updated Window").unwrap();
    assert_eq!(surface.label(&store), Some("Updated Window"));
    assert_eq!(store.get_node(id).unwrap().get_property(PropertyRef::LABEL), Some(&Value::String("Updated Window".into())));

    // Test clear
    surface.clear_label(&mut store).unwrap();
    assert_eq!(surface.label(&store), None);
    assert_eq!(store.get_node(id).unwrap().get_property(PropertyRef::LABEL), None);
}

#[test]
fn test_row_and_column_layout_widgets() {
    let mut store = SemanticStore::new();

    let row = Row::builder(201)
        .spacing_role(SpacingRole::Tight)
        .padding_role(PaddingRole::Normal)
        .horizontal_alignment(HorizontalAlignment::Leading)
        .vertical_alignment(VerticalAlignment::Center)
        .grow(2.0)
        .shrink(0.5)
        .preferred_size(Size::new(300.0, 50.0))
        .visibility(Visibility::Visible)
        .enabled(true)
        .busy(false)
        .accessible_description("Action buttons row")
        .create(&mut store)
        .expect("Failed to create Row");

    let col = Column::builder(202)
        .parent(row.id())
        .spacing_role(SpacingRole::Relaxed)
        .padding_role(PaddingRole::Tight)
        .horizontal_alignment(HorizontalAlignment::Center)
        .vertical_alignment(VerticalAlignment::Top)
        .create(&mut store)
        .expect("Failed to create Column");

    assert_eq!(row.spacing_role(&store), Some(SpacingRole::Tight));
    assert_eq!(row.grow(&store), Some(2.0));
    assert_eq!(col.spacing_role(&store), Some(SpacingRole::Relaxed));

    // Generic verification
    let row_node = store.get_node(NodeId::new(201)).unwrap();
    assert_eq!(row_node.node_type, TypeRef::ROW);
    assert_eq!(row_node.ordered_children, vec![NodeId::new(202)]);

    let col_node = store.get_node(NodeId::new(202)).unwrap();
    assert_eq!(col_node.node_type, TypeRef::COLUMN);
    assert_eq!(col_node.parent_id, Some(NodeId::new(201)));
}

#[test]
fn test_grid_widget() {
    let mut store = SemanticStore::new();

    let grid = Grid::builder(301)
        .columns(vec![Value::String("200px".into()), Value::String("1fr".into()), Value::String("auto".into())])
        .spacing_role(SpacingRole::Normal)
        .padding_role(PaddingRole::Normal)
        .create(&mut store)
        .expect("Failed to create Grid");

    assert_eq!(grid.columns(&store).map(|s| s.len()), Some(3));
    let node = store.get_node(grid.id()).unwrap();
    assert_eq!(node.node_type, TypeRef::GRID);
    assert_eq!(
        node.get_property(PropertyRef::COLUMNS),
        Some(&Value::List(vec![
            Value::String("200px".into()),
            Value::String("1fr".into()),
            Value::String("auto".into())
        ]))
    );
}

#[test]
fn test_spacer_and_separator_widgets() {
    let mut store = SemanticStore::new();

    let spacer = Spacer::builder(401)
        .grow(1.0)
        .preferred_size(Size::new(0.0, 16.0))
        .visibility(Visibility::Visible)
        .create(&mut store)
        .expect("Failed to create Spacer");

    let separator = Separator::builder(402)
        .horizontal_alignment(HorizontalAlignment::Fill)
        .enabled(true)
        .accessible_description("Section divider")
        .create(&mut store)
        .expect("Failed to create Separator");

    assert_eq!(spacer.grow(&store), Some(1.0));
    assert_eq!(separator.accessible_description(&store), Some("Section divider"));

    let spacer_node = store.get_node(spacer.id()).unwrap();
    assert_eq!(spacer_node.node_type, TypeRef::SPACER);

    let separator_node = store.get_node(separator.id()).unwrap();
    assert_eq!(separator_node.node_type, TypeRef::SEPARATOR);
}

#[test]
fn test_text_and_richtext_widgets_and_text_roles() {
    let mut store = SemanticStore::new();

    let text = Text::builder(501)
        .text("System Online")
        .role(TextRole::Status)
        .label("Status Indicator")
        .create(&mut store)
        .expect("Failed to create Text");

    let rich_text = RichText::builder(502)
        .text("Build 42f7ab failed")
        .role(TextRole::Error)
        .read_only(true)
        .create(&mut store)
        .expect("Failed to create RichText");

    assert_eq!(text.text(&store), Some("System Online"));
    assert_eq!(text.role(&store), Some(TextRole::Status));
    assert_eq!(rich_text.text(&store), Some("Build 42f7ab failed"));
    assert_eq!(rich_text.role(&store), Some(TextRole::Error));
    assert_eq!(rich_text.read_only(&store), Some(true));
    assert!(rich_text.is_read_only(&store));

    // Test importance on Text
    text.set_importance(&mut store, Importance::Emphasized).unwrap();
    assert_eq!(text.importance(&store), Some(Importance::Emphasized));
    assert_eq!(text.role(&store), None);

    // Test setting all TextRole variants
    for role in [
        TextRole::Title,
        TextRole::Heading,
        TextRole::Body,
        TextRole::Caption,
        TextRole::Code,
        TextRole::Status,
        TextRole::Warning,
        TextRole::Error,
    ] {
        text.set_role(&mut store, role).unwrap();
        assert_eq!(text.role(&store), Some(role));
        let node = store.get_node(text.id()).unwrap();
        assert_eq!(node.get_property(PropertyRef::ROLE), Some(&Value::EnumToken(EnumToken::from(role))));
    }

    let node_text = store.get_node(text.id()).unwrap();
    assert_eq!(node_text.node_type, TypeRef::TEXT);
    assert_eq!(node_text.get_property(PropertyRef::TEXT), Some(&Value::String("System Online".into())));

    let node_rich = store.get_node(rich_text.id()).unwrap();
    assert_eq!(node_rich.node_type, TypeRef::RICHTEXT);
}

#[test]
fn test_button_widget_and_action_roles() {
    let mut store = SemanticStore::new();

    let btn = Button::builder(601)
        .label("Delete Service")
        .role(ActionRole::Destructive)
        .action_key("delete_service")
        .actions(vec![Value::String("primary".into()), Value::String("context_menu".into())])
        .enabled(true)
        .busy(false)
        .selected(false)
        .create(&mut store)
        .expect("Failed to create Button");

    assert_eq!(btn.label(&store), Some("Delete Service"));
    assert_eq!(btn.role(&store), Some(ActionRole::Destructive));
    assert_eq!(btn.importance(&store), None);
    assert_eq!(btn.action_key(&store), Some("delete_service"));
    assert_eq!(btn.actions(&store).map(|a| a.len()), Some(2));
    assert!(btn.is_enabled(&store));
    assert!(!btn.is_busy(&store));

    // Test importance on Button
    btn.set_importance(&mut store, Importance::Emphasized).unwrap();
    assert_eq!(btn.importance(&store), Some(Importance::Emphasized));
    assert_eq!(btn.role(&store), None);

    // Test setting all ActionRole variants
    for role in [
        ActionRole::Normal,
        ActionRole::Primary,
        ActionRole::Destructive,
        ActionRole::Quiet,
    ] {
        btn.set_role(&mut store, role).unwrap();
        assert_eq!(btn.role(&store), Some(role));
        let node = store.get_node(btn.id()).unwrap();
        assert_eq!(node.get_property(PropertyRef::ROLE), Some(&Value::EnumToken(EnumToken::from(role))));
    }

    let node = store.get_node(btn.id()).unwrap();
    assert_eq!(node.node_type, TypeRef::BUTTON);
    assert_eq!(node.get_property(PropertyRef::LABEL), Some(&Value::String("Delete Service".into())));
    assert_eq!(node.get_property(PropertyRef::ACTION_KEY), Some(&Value::String("delete_service".into())));
}

#[test]
fn test_toggle_roundtrip_presentation_hints_and_values() {
    let mut store = SemanticStore::new();

    // 1. Construct via standard builder
    let t1 = Toggle::builder(701)
        .value(true)
        .presentation_hint(TogglePresentationHint::Automatic)
        .label("Enable Notifications")
        .create(&mut store)
        .expect("Failed to create Toggle");

    assert_eq!(t1.value(&store), Some(true));
    assert!(t1.is_value_set(&store));
    assert_eq!(t1.presentation_hint(&store), Some(TogglePresentationHint::Automatic));
    assert_eq!(t1.label(&store), Some("Enable Notifications"));

    let node1 = store.get_node(t1.id()).unwrap();
    assert_eq!(node1.node_type, TypeRef::TOGGLE);
    assert_eq!(node1.node_type, TypeRef::standard(12));
    assert_eq!(node1.get_property(PropertyRef::VALUE), Some(&Value::Bool(true)));
    assert_eq!(node1.get_property(PropertyRef::PRESENTATION_HINT), Some(&Value::EnumToken(EnumToken::new(5, 1))));

    // 2. Construct via convenience constructors
    let t_checkbox = Toggle::checkbox(702)
        .value(false)
        .label("Remember Me")
        .create(&mut store)
        .expect("Failed to create checkbox Toggle");

    let t_switch = Toggle::switch(703)
        .value(true)
        .label("Dark Mode")
        .create(&mut store)
        .expect("Failed to create switch Toggle");

    let t_auto = Toggle::automatic(704)
        .value(true)
        .label("Auto Sync")
        .create(&mut store)
        .expect("Failed to create auto Toggle");

    assert_eq!(t_checkbox.presentation_hint(&store), Some(TogglePresentationHint::Checkbox));
    assert_eq!(t_switch.presentation_hint(&store), Some(TogglePresentationHint::Switch));
    assert_eq!(t_auto.presentation_hint(&store), Some(TogglePresentationHint::Automatic));

    // Confirm that all 4 toggles have node_type == TypeRef::TOGGLE (no separate Checkbox/Switch node type)
    for id in [701, 702, 703, 704] {
        let node = store.get_node(NodeId::new(id)).unwrap();
        assert_eq!(node.node_type, TypeRef::TOGGLE, "Node {} must have TypeRef::TOGGLE", id);
        assert_eq!(node.node_type, TypeRef::standard(12));
    }

    // 3. Test round-tripping both presentation_hint values dynamically
    t_checkbox.set_presentation_hint(&mut store, TogglePresentationHint::Switch).unwrap();
    assert_eq!(t_checkbox.presentation_hint(&store), Some(TogglePresentationHint::Switch));
    let n = store.get_node(t_checkbox.id()).unwrap();
    assert_eq!(n.get_property(PropertyRef::PRESENTATION_HINT), Some(&Value::EnumToken(EnumToken::new(5, 3))));

    t_checkbox.set_presentation_hint(&mut store, TogglePresentationHint::Checkbox).unwrap();
    assert_eq!(t_checkbox.presentation_hint(&store), Some(TogglePresentationHint::Checkbox));
    let n = store.get_node(t_checkbox.id()).unwrap();
    assert_eq!(n.get_property(PropertyRef::PRESENTATION_HINT), Some(&Value::EnumToken(EnumToken::new(5, 2))));

    // 4. Test validation_state and read_only
    t_switch.set_validation_state(&mut store, ValidationState::Warning).unwrap();
    assert_eq!(t_switch.validation_state(&store), Some(ValidationState::Warning));

    t_switch.set_read_only(&mut store, true).unwrap();
    assert_eq!(t_switch.read_only(&store), Some(true));
    assert!(t_switch.is_read_only(&store));
}

#[test]
fn test_text_input_and_text_area_and_input_roles() {
    let mut store = SemanticStore::new();

    let input = TextInput::builder(801)
        .value("secret_token")
        .placeholder("Enter API token...")
        .role(InputRole::Secure)
        .label("API Token")
        .action_key("api_token_field")
        .create(&mut store)
        .expect("Failed to create TextInput");

    let area = TextArea::builder(802)
        .value("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5...")
        .placeholder("Paste public keys here...")
        .role(InputRole::Plain)
        .label("SSH Keys")
        .create(&mut store)
        .expect("Failed to create TextArea");

    assert_eq!(input.value(&store), Some("secret_token"));
    assert_eq!(input.placeholder(&store), Some("Enter API token..."));
    assert_eq!(input.role(&store), Some(InputRole::Secure));

    assert_eq!(area.value(&store), Some("ssh-ed25519 AAAAC3NzaC1lZDI1NTE5..."));
    assert_eq!(area.role(&store), Some(InputRole::Plain));

    // Test all InputRole variants
    for role in [
        InputRole::Plain,
        InputRole::Search,
        InputRole::Secure,
        InputRole::Command,
    ] {
        input.set_role(&mut store, role).unwrap();
        assert_eq!(input.role(&store), Some(role));
        let node = store.get_node(input.id()).unwrap();
        assert_eq!(node.get_property(PropertyRef::ROLE), Some(&Value::EnumToken(EnumToken::from(role))));
    }

    let input_node = store.get_node(input.id()).unwrap();
    assert_eq!(input_node.node_type, TypeRef::TEXT_INPUT);

    let area_node = store.get_node(area.id()).unwrap();
    assert_eq!(area_node.node_type, TypeRef::TEXT_AREA);
}

#[test]
fn test_progress_widget() {
    let mut store = SemanticStore::new();

    let progress = Progress::builder(901)
        .value(0.72)
        .value_description("72% complete")
        .label("Download Progress")
        .busy(true)
        .create(&mut store)
        .expect("Failed to create Progress");

    assert_eq!(progress.value(&store), Some(0.72));
    assert_eq!(progress.value_description(&store), Some("72% complete"));
    assert!(progress.is_busy(&store));

    {
        let node = store.get_node(progress.id()).unwrap();
        assert_eq!(node.node_type, TypeRef::PROGRESS);
        assert_eq!(node.get_property(PropertyRef::VALUE), Some(&Value::Float64(0.72)));
        assert_eq!(node.get_property(PropertyRef::VALUE_DESCRIPTION), Some(&Value::String("72% complete".into())));
    }

    // Update progress scalar
    progress.set_value(&mut store, 0.73).unwrap();
    assert_eq!(progress.value(&store), Some(0.73));
    assert_eq!(store.get_node(progress.id()).unwrap().get_property(PropertyRef::VALUE), Some(&Value::Float64(0.73)));
}

#[test]
fn test_image_widget_and_resource_hash() {
    let mut store = SemanticStore::new();
    let hash_bytes = [0x42u8; 32];
    let res_hash = ResourceHash::new(hash_bytes);

    let image = Image::builder(1001)
        .resource(res_hash)
        .label("Logo")
        .accessible_description("Company Logo Image")
        .preferred_size(Size::new(128.0, 128.0))
        .create(&mut store)
        .expect("Failed to create Image");

    assert_eq!(image.resource(&store), Some(res_hash));
    assert_eq!(image.label(&store), Some("Logo"));
    assert_eq!(image.preferred_size(&store), Some(Size::new(128.0, 128.0)));

    let node = store.get_node(image.id()).unwrap();
    assert_eq!(node.node_type, TypeRef::IMAGE);
    assert_eq!(node.get_property(PropertyRef::RESOURCE), Some(&Value::ResourceHash(res_hash)));
}

#[test]
fn test_scroll_widget() {
    let mut store = SemanticStore::new();

    let scroll = Scroll::builder(1101)
        .spacing_role(SpacingRole::Normal)
        .padding_role(PaddingRole::Tight)
        .horizontal_alignment(HorizontalAlignment::Fill)
        .vertical_alignment(VerticalAlignment::Fill)
        .create(&mut store)
        .expect("Failed to create Scroll");

    assert_eq!(scroll.spacing_role(&store), Some(SpacingRole::Normal));
    assert_eq!(scroll.padding_role(&store), Some(PaddingRole::Tight));

    let node = store.get_node(scroll.id()).unwrap();
    assert_eq!(node.node_type, TypeRef::SCROLL);
}

#[test]
fn test_list_table_tree_collection_widgets() {
    let mut store = SemanticStore::new();

    // Create models first in store so model_ref validation passes (§8)
    let model_id1 = ModelId::new(1);
    let model_id2 = ModelId::new(2);
    let model_id3 = ModelId::new(3);

    store.create_model(model_id1, TypeRef::LIST, 100).unwrap();
    store.create_model(model_id2, TypeRef::TABLE, 500).unwrap();
    store.create_model(model_id3, TypeRef::TREE, 250).unwrap();

    let list = List::builder(1201)
        .model_ref(model_id1)
        .selection_mode(SelectionMode::Single)
        .selected(true)
        .label("Services List")
        .create(&mut store)
        .expect("Failed to create List");

    let table = Table::builder(1202)
        .model_ref(model_id2)
        .columns(vec![
            Value::String("Service Name".into()),
            Value::String("Status".into()),
            Value::String("Uptime".into()),
        ])
        .selection_mode(SelectionMode::Multiple)
        .label("Processes Table")
        .create(&mut store)
        .expect("Failed to create Table");

    let tree = Tree::builder(1203)
        .model_ref(model_id3)
        .selection_mode(SelectionMode::Single)
        .label("Filesystem Tree")
        .create(&mut store)
        .expect("Failed to create Tree");

    assert_eq!(list.model_ref(&store), Some(model_id1));
    assert_eq!(list.selection_mode(&store), Some(SelectionMode::Single));
    assert!(list.is_selected(&store));

    assert_eq!(table.model_ref(&store), Some(model_id2));
    assert_eq!(table.columns(&store).map(|c| c.len()), Some(3));
    assert_eq!(table.selection_mode(&store), Some(SelectionMode::Multiple));

    assert_eq!(tree.model_ref(&store), Some(model_id3));
    assert_eq!(tree.selection_mode(&store), Some(SelectionMode::Single));

    let list_node = store.get_node(list.id()).unwrap();
    assert_eq!(list_node.node_type, TypeRef::LIST);
    assert_eq!(list_node.get_property(PropertyRef::MODEL_REF), Some(&Value::UnsignedInt(1)));

    let table_node = store.get_node(table.id()).unwrap();
    assert_eq!(table_node.node_type, TypeRef::TABLE);
    assert_eq!(table_node.get_property(PropertyRef::MODEL_REF), Some(&Value::UnsignedInt(2)));

    let tree_node = store.get_node(tree.id()).unwrap();
    assert_eq!(tree_node.node_type, TypeRef::TREE);
    assert_eq!(tree_node.get_property(PropertyRef::MODEL_REF), Some(&Value::UnsignedInt(3)));
}

#[test]
fn test_builder_into_operation_and_atomic_transactions() {
    let mut store = SemanticStore::new();

    let root_op = Surface::builder(1)
        .label("Dashboard")
        .into_operation();

    let row_op = Row::builder(2)
        .parent(1)
        .spacing_role(SpacingRole::Normal)
        .into_operation();

    let btn_op = Button::builder(3)
        .parent(2)
        .label("Approve")
        .role(ActionRole::Primary)
        .into_operation();

    let tgl_op = Toggle::switch(4)
        .parent(2)
        .value(true)
        .label("Live Monitoring")
        .into_operation();

    let rev = store
        .apply_transaction(0, vec![root_op, row_op, btn_op, tgl_op])
        .expect("Transaction failed");

    assert_eq!(rev.get(), 1);
    assert_eq!(store.node_count(), 4);

    let btn = Button::from_store(&store, NodeId::new(3)).expect("Button not found");
    assert_eq!(btn.label(&store), Some("Approve"));
    assert_eq!(btn.role(&store), Some(ActionRole::Primary));

    let tgl = Toggle::from_store(&store, NodeId::new(4)).expect("Toggle not found");
    assert_eq!(tgl.value(&store), Some(true));
    assert_eq!(tgl.presentation_hint(&store), Some(TogglePresentationHint::Switch));
}

#[test]
fn test_widget_trait_and_generic_delete() {
    let mut store = SemanticStore::new();

    let surface = Surface::builder(1).create(&mut store).unwrap();
    let row = Row::builder(2).parent(1).create(&mut store).unwrap();
    let btn = Button::builder(3).parent(2).label("Click").create(&mut store).unwrap();

    assert_eq!(store.node_count(), 3);
    assert_eq!(btn.id(), NodeId::new(3));
    assert_eq!(btn.node_type(), TypeRef::BUTTON);

    // Delete row (should recursively delete row and btn)
    let deleted = row.delete(&mut store).expect("Delete failed");
    assert_eq!(deleted.len(), 2);
    assert_eq!(store.node_count(), 1);
    assert!(store.contains_node(surface.id()));
    assert!(!store.contains_node(row.id()));
    assert!(!store.contains_node(btn.id()));
}
