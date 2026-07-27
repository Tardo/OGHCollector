// Copyright Alexandre D. Díaz
//! Manual smoke check for the model path (downloads the model on first run):
//! `cargo run -p oghembed --example smoke`
fn main() {
    let texts = [
        "Sale Order Type (sale_order_type). Category: Sales. Manage different types of sale orders",
        "Fleet Vehicle Maintenance (fleet_maintenance). Category: Fleet. Track vehicle repairs",
    ];
    let vectors = oghembed::embed_texts(&texts).expect("embedding failed");
    let query = oghembed::embed_texts(&["gestionar pedidos de venta"]).expect("embedding failed");
    let s0 = oghembed::cosine(&query[0], &vectors[0]);
    let s1 = oghembed::cosine(&query[0], &vectors[1]);
    println!("dims={} sale={s0:.3} fleet={s1:.3}", vectors[0].len());
    assert_eq!(vectors[0].len(), 384);
    assert!(
        s0 > s1,
        "Spanish sales query should rank the sales module first"
    );
    println!("OK");
}
