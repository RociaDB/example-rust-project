//! The graph service: who supplies what, which quote became which invoice.
//!
//! Two things shape this module:
//!
//! - **The graph is an index, not the source of truth.** No RPC reads an
//!   edge's value back — `neighbors_out` returns a `node_id` and an
//!   `edge_id`, nothing else. Anything you need to read lives in the
//!   document.
//! - **An edge needs both endpoints first.** `add_edge` returns `NOT_FOUND`
//!   if `from` or `to` is not already a node. Nodes first, edges after.

use crate::model::Product;
use crate::{Erp, GRAPH, Result};
use rocia_db_sdk::{EdgeInput, Neighbor, NodeInput};
use serde::{Deserialize, Serialize};
use serde_json::json;

// Edge labels.
pub const SUPPLIES: &str = "supplies";
pub const REQUESTED: &str = "requested";
pub const CONVERTED_TO: &str = "converted_to";
pub const BILLED_AS: &str = "billed_as";

/// The value `create_document` writes into the node it binds: a pointer back
/// to the document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocRef {
    pub collection: String,
    pub id: String,
}

/// A node holding the pointer plus a couple of denormalized fields, so a
/// traversal can show something readable without re-reading each document.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProductNode {
    pub collection: String,
    pub id: String,
    pub reference: String,
    pub name: String,
}

/// Node ids follow `"{label}:{id}"` — the same shape `create_document` uses.
pub fn node(label: &str, id: &str) -> String {
    format!("{label}:{id}")
}

/// Edge ids are ours to choose; `delete_edge` takes only this id, so it has
/// to be rebuildable without reading the graph first.
pub fn edge(label: &str, from: &str, to: &str) -> String {
    format!("{label}|{from}|{to}")
}

/// Write all product nodes in one batch.
///
/// `put_nodes` sends up to 10 requests concurrently. A node's value must be
/// a JSON **object** — a scalar or an array is rejected.
pub async fn put_product_nodes(erp: &Erp, products: &[Product]) -> Result<()> {
    let mut nodes = Vec::new();
    for product in products {
        nodes.push(NodeInput {
            node_id: node("product", &product.id),
            value: serde_json::to_value(ProductNode {
                collection: crate::documents::PRODUCTS.to_string(),
                id: product.id.clone(),
                reference: product.reference.clone(),
                name: product.name.clone(),
            })?,
            request_id: Some(erp.key(&format!("node:{}", product.id))),
        });
    }
    erp.client.put_nodes(&erp.tenant, GRAPH, nodes).await?;
    Ok(())
}

/// Write one node. `put_node` generates its own idempotency key.
pub async fn put_node(erp: &Erp, node_id: &str, value: &DocRef) -> Result<()> {
    erp.client
        .put_node(&erp.tenant, GRAPH, node_id, value)
        .await?;
    Ok(())
}

/// Write one node with a chosen key — what you would use to repair a
/// document whose node binding never made it.
pub async fn put_node_with_key(erp: &Erp, node_id: &str, value: &DocRef) -> Result<()> {
    erp.client
        .put_node_with_request_id(
            &erp.tenant,
            GRAPH,
            node_id,
            value,
            erp.key(&format!("repair:{node_id}")),
        )
        .await?;
    Ok(())
}

/// Link a supplier to the products it supplies, in one batch.
///
/// The edge value records the purchase terms. It is useful when reading the
/// data server-side, but the SDK cannot read it back.
pub async fn link_supplier_products(
    erp: &Erp,
    supplier_id: &str,
    products: &[(&str, i64)],
) -> Result<()> {
    let from = node("supplier", supplier_id);
    let edges: Vec<EdgeInput> = products
        .iter()
        .map(|(product_id, purchase_price)| {
            let to = node("product", product_id);
            EdgeInput {
                edge_id: edge(SUPPLIES, &from, &to),
                from: from.clone(),
                to,
                label: SUPPLIES.to_string(),
                value: json!({ "purchase_price": purchase_price }),
                request_id: Some(erp.key(&format!("supplies:{supplier_id}:{product_id}"))),
            }
        })
        .collect();

    erp.client.add_edges(&erp.tenant, GRAPH, edges).await?;
    Ok(())
}

/// Add one edge. The SDK generates the idempotency key.
pub async fn link(erp: &Erp, label: &str, from: &str, to: &str) -> Result<()> {
    erp.client
        .add_edge(
            &erp.tenant,
            GRAPH,
            &edge(label, from, to),
            from,
            to,
            label,
            &json!({ "note": "created by the demo" }),
        )
        .await?;
    Ok(())
}

/// Add one edge with a chosen key, so a retry after a timeout does not
/// create a second one.
pub async fn link_with_key(erp: &Erp, label: &str, from: &str, to: &str) -> Result<()> {
    erp.client
        .add_edge_with_request_id(
            &erp.tenant,
            GRAPH,
            &edge(label, from, to),
            from,
            to,
            label,
            &json!({ "note": "created by the demo" }),
            erp.key(&format!("{label}:{from}:{to}")),
        )
        .await?;
    Ok(())
}

/// The products a supplier supplies, node values included.
///
/// `get_outgoing_neighbor_nodes` does in one call what `neighbors_out` plus
/// a `get_node` per result would do. Because product nodes carry the name,
/// nothing else has to be read.
pub async fn products_of_supplier(erp: &Erp, supplier_id: &str) -> Result<Vec<ProductNode>> {
    let neighbors = erp
        .client
        .get_outgoing_neighbor_nodes::<ProductNode>(
            &erp.tenant,
            GRAPH,
            &node("supplier", supplier_id),
            SUPPLIES,
        )
        .await?;
    Ok(neighbors.into_iter().map(|n| n.value).collect())
}

/// Who supplies a product: the same traversal, backwards.
pub async fn suppliers_of_product(erp: &Erp, product_id: &str) -> Result<Vec<String>> {
    let neighbors = erp
        .client
        .get_incoming_neighbor_nodes::<DocRef>(
            &erp.tenant,
            GRAPH,
            &node("product", product_id),
            SUPPLIES,
        )
        .await?;
    Ok(neighbors.into_iter().map(|n| n.value.id).collect())
}

/// Raw outgoing neighbors. Prefer this over the typed helper above once a
/// node has many edges: this one paginates, that one returns everything.
pub async fn neighbors_out(erp: &Erp, node_id: &str, label: &str) -> Result<Vec<Neighbor>> {
    let page = erp
        .client
        .neighbors_out(&erp.tenant, GRAPH, node_id, label, Some(50), None)
        .await?;
    Ok(page.neighbors)
}

/// Raw incoming neighbors.
pub async fn neighbors_in(erp: &Erp, node_id: &str, label: &str) -> Result<Vec<Neighbor>> {
    let page = erp
        .client
        .neighbors_in(&erp.tenant, GRAPH, node_id, label, Some(50), None)
        .await?;
    Ok(page.neighbors)
}

/// A node as stored, without forcing it into a Rust type. Useful when you do
/// not know its shape; `get_node_as::<T>` is the same call with decoding.
pub async fn raw_node(erp: &Erp, node_id: &str) -> Result<serde_json::Value> {
    Ok(erp.client.get_node(&erp.tenant, GRAPH, node_id).await?)
}

/// The `{collection, id}` pointer a bound node carries.
pub async fn node_ref(erp: &Erp, node_id: &str) -> Result<DocRef> {
    Ok(erp
        .client
        .get_node_as::<DocRef>(&erp.tenant, GRAPH, node_id)
        .await?)
}

/// The graphs in this tenant. Like collections, one exists as soon as a node
/// is written to it.
pub async fn list_graphs(erp: &Erp) -> Result<Vec<String>> {
    let page = erp.client.list_graphs(&erp.tenant, Some(50), None).await?;
    Ok(page.items)
}

/// Every node id in the graph.
pub async fn list_nodes(erp: &Erp) -> Result<Vec<String>> {
    let page = erp
        .client
        .list_nodes(&erp.tenant, GRAPH, Some(200), None)
        .await?;
    Ok(page.items)
}

/// Delete an edge.
///
/// This is **not** idempotent: a missing edge returns `NOT_FOUND`, unlike
/// `delete_document` and `delete_file`.
pub async fn unlink(erp: &Erp, edge_id: &str) -> Result<()> {
    erp.client.delete_edge(&erp.tenant, GRAPH, edge_id).await?;
    Ok(())
}

/// Delete an edge with a chosen key.
pub async fn unlink_with_key(erp: &Erp, edge_id: &str) -> Result<()> {
    erp.client
        .delete_edge_with_request_id(
            &erp.tenant,
            GRAPH,
            edge_id,
            erp.key(&format!("unlink:{edge_id}")),
        )
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn node_ids_follow_the_sdk_convention() {
        // `create_document` builds exactly "{label}:{id}". If that changed,
        // every traversal here would look at the wrong node.
        assert_eq!(node("customer", "C-1"), "customer:C-1");
        assert_eq!(node("invoice", "INV-2026-1"), "invoice:INV-2026-1");
    }

    #[test]
    fn edge_ids_are_rebuildable() {
        assert_eq!(
            edge(SUPPLIES, "supplier:S-1", "product:P-1"),
            "supplies|supplier:S-1|product:P-1"
        );
    }
}
