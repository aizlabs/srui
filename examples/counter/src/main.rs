//! Runnable binary demonstrating the SRUI Counter Example (§29).

use srui_example_counter::CounterApp;

fn main() {
    println!("=== SRUI Counter Example ===");
    let app = CounterApp::new().expect("failed to initialize CounterApp");

    println!("Initial UI State (Revision {}):", app.current_revision());
    println!("  Text:     {}", app.get_text().unwrap_or_default());
    println!("  Progress: {:.2} ({})", app.get_progress().unwrap_or(0.0), app.get_progress_description().unwrap_or_default());

    // Dispatch 5 simulated button clicks
    for seq in 1..=5 {
        app.click(seq).expect("click dispatch failed");
        println!("After Click {} (Revision {}):", seq, app.current_revision());
        println!("  Text:     {}", app.get_text().unwrap_or_default());
        println!("  Progress: {:.2} ({})", app.get_progress().unwrap_or(0.0), app.get_progress_description().unwrap_or_default());
    }

    println!("=== Counter Example completed successfully! ===");
}
