use std::env;
use std::fs;
use std::path::Path;

#[derive(Debug)]
struct Item {
    id: u32,
    name: String,
}

#[derive(Debug)]
#[allow(dead_code)]
struct EnumDef {
    id: u32,
    name: String,
    values: Vec<Item>,
}

fn parse_simple_section(lines: &[&str], start_header: &str, end_headers: &[&str]) -> Vec<Item> {
    let mut items = Vec::new();
    let mut in_section = false;
    let mut current_id: Option<u32> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if !in_section {
            if trimmed.starts_with(start_header) {
                in_section = true;
            }
            continue;
        }

        // Check if we reached another top-level section
        if !line.starts_with(' ') && !line.starts_with('\t') {
            for end_h in end_headers {
                if trimmed.starts_with(end_h) {
                    in_section = false;
                    break;
                }
            }
            if !in_section {
                break;
            }
        }

        if let Some(rest) = trimmed.strip_prefix("- id:") {
            if let Ok(id) = rest.trim().parse::<u32>() {
                current_id = Some(id);
            }
        } else if let Some(rest) = trimmed.strip_prefix("id:") {
            if let Ok(id) = rest.trim().parse::<u32>() {
                current_id = Some(id);
            }
        }

        if let Some(rest) = trimmed.strip_prefix("name:") {
            let name = rest.trim().trim_matches('"').trim_matches('\'').to_string();
            if let Some(id) = current_id.take() {
                items.push(Item { id, name });
            }
        }
    }

    items
}

fn parse_enums(lines: &[&str]) -> Vec<EnumDef> {
    let mut enums = Vec::new();
    let mut in_enums = false;
    let mut current_enum_id: Option<u32> = None;
    let mut current_enum_name: Option<String> = None;
    let mut current_values = Vec::new();
    let mut in_values = false;
    let mut current_value_id: Option<u32> = None;

    for line in lines {
        let trimmed = line.trim();
        if trimmed.starts_with('#') || trimmed.is_empty() {
            continue;
        }

        if !in_enums {
            if trimmed.starts_with("enums:") {
                in_enums = true;
            }
            continue;
        }

        if !line.starts_with(' ') && !line.starts_with('\t') {
            if trimmed.starts_with("events:") || trimmed.starts_with("operations:") {
                break;
            }
        }

        // Detect new top-level enum entry
        let leading_spaces = line.chars().take_while(|c| *c == ' ').count();
        if leading_spaces == 2 && trimmed.starts_with("- id:") {
            // Flush previous enum
            if let (Some(id), Some(name)) = (current_enum_id.take(), current_enum_name.take()) {
                enums.push(EnumDef {
                    id,
                    name,
                    values: std::mem::take(&mut current_values),
                });
            }
            in_values = false;
            if let Ok(id) = trimmed.strip_prefix("- id:").unwrap().trim().parse::<u32>() {
                current_enum_id = Some(id);
            }
            continue;
        }

        if leading_spaces == 4 && trimmed.starts_with("name:") && current_enum_name.is_none() && !in_values {
            current_enum_name = Some(trimmed.strip_prefix("name:").unwrap().trim().trim_matches('"').to_string());
            continue;
        }

        if trimmed.starts_with("values:") {
            in_values = true;
            continue;
        }

        if in_values {
            if leading_spaces == 6 && trimmed.starts_with("- id:") {
                if let Ok(id) = trimmed.strip_prefix("- id:").unwrap().trim().parse::<u32>() {
                    current_value_id = Some(id);
                }
            } else if leading_spaces == 8 && trimmed.starts_with("name:") {
                let vname = trimmed.strip_prefix("name:").unwrap().trim().trim_matches('"').to_string();
                if let Some(vid) = current_value_id.take() {
                    current_values.push(Item { id: vid, name: vname });
                }
            }
        }
    }

    if let (Some(id), Some(name)) = (current_enum_id.take(), current_enum_name.take()) {
        enums.push(EnumDef {
            id,
            name,
            values: current_values,
        });
    }

    enums
}

fn main() {
    let manifest_dir = env::var("CARGO_MANIFEST_DIR").unwrap();
    let registry_path = Path::new(&manifest_dir).join("../../protocol/registry.yaml");

    println!("cargo:rerun-if-changed={}", registry_path.display());
    println!("cargo:rerun-if-changed=build.rs");

    let content = fs::read_to_string(&registry_path)
        .unwrap_or_else(|e| panic!("Failed to read registry.yaml at {}: {}", registry_path.display(), e));

    let lines: Vec<&str> = content.lines().collect();

    let node_types = parse_simple_section(&lines, "node_types:", &["properties:", "enums:", "events:", "operations:"]);
    let properties = parse_simple_section(&lines, "properties:", &["enums:", "events:", "operations:"]);
    let enums = parse_enums(&lines);
    let events = parse_simple_section(&lines, "events:", &["operations:"]);
    let operations = parse_simple_section(&lines, "operations:", &[]);

    assert_eq!(node_types.len(), 27, "Expected 27 standard node types, parsed {}", node_types.len());
    assert_eq!(properties.len(), 30, "Expected 30 standard properties, parsed {}", properties.len());
    assert_eq!(enums.len(), 12, "Expected 12 standard enums, parsed {}", enums.len());
    assert_eq!(events.len(), 11, "Expected 11 standard events, parsed {}", events.len());
    assert_eq!(operations.len(), 13, "Expected 13 standard operations, parsed {}", operations.len());

    let mut code = String::new();
    code.push_str("// Auto-generated standard registry lookup tables from protocol/registry.yaml\n");
    code.push_str("// Generated by semantic-tree/build.rs. Do not edit manually.\n\n");

    // Standard Node Types table
    code.push_str("pub static STANDARD_NODE_TYPES: &[(u32, &str)] = &[\n");
    for item in &node_types {
        code.push_str(&format!("    ({}, {:?}),\n", item.id, item.name));
    }
    code.push_str("];\n\n");

    // Standard Properties table
    code.push_str("pub static STANDARD_PROPERTIES: &[(u32, &str)] = &[\n");
    for item in &properties {
        code.push_str(&format!("    ({}, {:?}),\n", item.id, item.name));
    }
    code.push_str("];\n\n");

    // Standard Events table
    code.push_str("pub static STANDARD_EVENTS: &[(u32, &str)] = &[\n");
    for item in &events {
        code.push_str(&format!("    ({}, {:?}),\n", item.id, item.name));
    }
    code.push_str("];\n\n");

    // Standard Operations table
    code.push_str("pub static STANDARD_OPERATIONS: &[(u32, &str)] = &[\n");
    for item in &operations {
        code.push_str(&format!("    ({}, {:?}),\n", item.id, item.name));
    }
    code.push_str("];\n\n");

    // Standard Enums table
    code.push_str("pub static STANDARD_ENUMS: &[(u32, &str)] = &[\n");
    for item in &enums {
        code.push_str(&format!("    ({}, {:?}),\n", item.id, item.name));
    }
    code.push_str("];\n\n");

    // Lookup functions for Node Types
    code.push_str("pub fn lookup_standard_node_type(name: &str) -> Option<u32> {\n");
    code.push_str("    match name {\n");
    for item in &node_types {
        code.push_str(&format!("        {:?} => Some({}),\n", item.name, item.id));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("pub fn standard_node_type_name(id: u32) -> Option<&'static str> {\n");
    code.push_str("    match id {\n");
    for item in &node_types {
        code.push_str(&format!("        {} => Some({:?}),\n", item.id, item.name));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    // Lookup functions for Properties
    code.push_str("pub fn lookup_standard_property(name: &str) -> Option<u32> {\n");
    code.push_str("    match name {\n");
    for item in &properties {
        code.push_str(&format!("        {:?} => Some({}),\n", item.name, item.id));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("pub fn standard_property_name(id: u32) -> Option<&'static str> {\n");
    code.push_str("    match id {\n");
    for item in &properties {
        code.push_str(&format!("        {} => Some({:?}),\n", item.id, item.name));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    // Lookup functions for Events
    code.push_str("pub fn lookup_standard_event(name: &str) -> Option<u32> {\n");
    code.push_str("    match name {\n");
    for item in &events {
        code.push_str(&format!("        {:?} => Some({}),\n", item.name, item.id));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("pub fn standard_event_name(id: u32) -> Option<&'static str> {\n");
    code.push_str("    match id {\n");
    for item in &events {
        code.push_str(&format!("        {} => Some({:?}),\n", item.id, item.name));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    // Lookup functions for Operations
    code.push_str("pub fn lookup_standard_operation(name: &str) -> Option<u32> {\n");
    code.push_str("    match name {\n");
    for item in &operations {
        code.push_str(&format!("        {:?} => Some({}),\n", item.name, item.id));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("pub fn standard_operation_name(id: u32) -> Option<&'static str> {\n");
    code.push_str("    match id {\n");
    for item in &operations {
        code.push_str(&format!("        {} => Some({:?}),\n", item.id, item.name));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    // Lookup functions for Enums
    code.push_str("pub fn lookup_standard_enum(name: &str) -> Option<u32> {\n");
    code.push_str("    match name {\n");
    for item in &enums {
        code.push_str(&format!("        {:?} => Some({}),\n", item.name, item.id));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    code.push_str("pub fn standard_enum_name(id: u32) -> Option<&'static str> {\n");
    code.push_str("    match id {\n");
    for item in &enums {
        code.push_str(&format!("        {} => Some({:?}),\n", item.id, item.name));
    }
    code.push_str("        _ => None,\n");
    code.push_str("    }\n");
    code.push_str("}\n\n");

    let out_dir = env::var("OUT_DIR").unwrap();
    let dest_path = Path::new(&out_dir).join("registry_tables.rs");
    fs::write(&dest_path, code).unwrap_or_else(|e| panic!("Failed to write registry_tables.rs: {}", e));
}
