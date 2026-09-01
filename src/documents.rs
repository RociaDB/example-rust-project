//! The document service: write, read, search, query, delete.

use crate::model::*;
use crate::{Erp, GRAPH, Result};
use rociadb_sdk::{
    DocumentQueryFilter, DocumentQueryOperator, DocumentQuerySort, DocumentQuerySortDirection,
};
use serde::de::DeserializeOwned;
use serde_json::json;

// Collections are never declared: one appears the first time a document is
// written to it. Constants keep a typo from silently creating another one.
pub const CUSTOMERS: &str = "customers";
pub const SUPPLIERS: &str = "suppliers";
pub const PRODUCTS: &str = "products";
pub const QUOTES: &str = "quotes";
pub const ORDERS: &str = "orders";
pub const INVOICES: &str = "invoices";
pub const STOCK_MOVES: &str = "stock_moves";

/// Write a customer, plus the graph node pointing back at it.
///
/// `create_document` does two things: it writes the document, then, when
/// `node_label` and `node_graph` are both given, it upserts the node
/// `"{label}:{id}"` holding a `{collection, id}` pointer to the document.
///
/// The two writes are not atomic. If the node write fails, the document is
/// left without its binding.
pub async fn create_customer(erp: &Erp, customer: &Customer) -> Result<()> {
    erp.client
        .create_document(
            &erp.tenant,
            CUSTOMERS,
            &customer.id,
            serde_json::to_value(customer)?,
            Some("customer".to_string()),
            Some(GRAPH.to_string()),
        )
        .await?;
    Ok(())
}

/// Same thing with an idempotency key we choose.
///
/// The server deduplicates on `(tenant, operation, request_id)` for 24 hours,
/// so replaying an interrupted import with the same keys is safe. Note the
/// key covers only the document write: the node binding generates its own.
pub async fn create_supplier(erp: &Erp, supplier: &Supplier) -> Result<()> {
    erp.client
        .create_document_with_request_id(
            &erp.tenant,
            SUPPLIERS,
            &supplier.id,
            supplier,
            Some("supplier".to_string()),
            Some(GRAPH.to_string()),
            erp.key(&format!("supplier:{}", supplier.id)),
        )
        .await?;
    Ok(())
}

/// Write a product. `put_document` writes the document only, no graph node.
pub async fn save_product(erp: &Erp, product: &Product) -> Result<()> {
    erp.client
        .put_document(&erp.tenant, PRODUCTS, &product.id, product)
        .await?;
    Ok(())
}

/// Same, with a stable key: this is the replayable-import case.
pub async fn import_product(erp: &Erp, product: &Product) -> Result<()> {
    erp.client
        .put_document_with_request_id(
            &erp.tenant,
            PRODUCTS,
            &product.id,
            product,
            erp.key(&format!("import:{}", product.id)),
        )
        .await?;
    Ok(())
}

/// Read one document into the requested type.
///
/// A stored document that no longer matches the Rust type fails with
/// `RociaDbError::Decode`, not a `Value` to inspect by hand.
pub async fn get<T: DeserializeOwned>(erp: &Erp, collection: &str, id: &str) -> Result<T> {
    Ok(erp
        .client
        .get_document::<T>(&erp.tenant, collection, id)
        .await?)
}

/// Write any business document (quote, order, invoice, stock move).
pub async fn put<T: serde::Serialize>(
    erp: &Erp,
    collection: &str,
    id: &str,
    value: &T,
) -> Result<()> {
    erp.client
        .put_document(&erp.tenant, collection, id, value)
        .await?;
    Ok(())
}

/// Write a document and bind it to a graph node, with a chosen key.
pub async fn create_bound<T: serde::Serialize>(
    erp: &Erp,
    collection: &str,
    label: &str,
    id: &str,
    value: &T,
) -> Result<()> {
    erp.client
        .create_document_with_request_id(
            &erp.tenant,
            collection,
            id,
            value,
            Some(label.to_string()),
            Some(GRAPH.to_string()),
            erp.key(&format!("{collection}:{id}")),
        )
        .await?;
    Ok(())
}

/// Every product, page after page.
///
/// This is the cursor pattern: the cursor is opaque, you pass it back
/// unchanged, and it is `None` once the server has nothing more. The
/// `total_count` that comes with it is free here, because the server keeps a
/// per-collection counter.
pub async fn all_products(erp: &Erp) -> Result<Vec<Product>> {
    let mut products = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let page = erp
            .client
            .list_documents::<Product>(&erp.tenant, PRODUCTS, Some(50), cursor.as_deref())
            .await?;
        products.extend(page.items);
        match page.next_cursor {
            Some(next) => cursor = Some(next),
            None => return Ok(products),
        }
    }
}

/// How many customers, without fetching them.
pub async fn count_customers(erp: &Erp) -> Result<u64> {
    let page = erp
        .client
        .list_documents::<serde_json::Value>(&erp.tenant, CUSTOMERS, Some(1), None)
        .await?;
    Ok(page.total_count)
}

/// Find customers by exact e-mail.
///
/// `search_documents` matches one field exactly, and the value must
/// serialize to a JSON scalar (string, number, bool, null). An object or an
/// array is rejected with `INVALID_ARGUMENT`.
pub async fn find_customer_by_email(erp: &Erp, email: &str) -> Result<Vec<Customer>> {
    let page = erp
        .client
        .search_documents::<Customer>(&erp.tenant, CUSTOMERS, "email", &email, Some(20), None)
        .await?;
    Ok(page.items)
}

/// Stock moves for one product.
pub async fn moves_for_product(erp: &Erp, product_id: &str) -> Result<Vec<StockMove>> {
    let page = erp
        .client
        .search_documents::<StockMove>(
            &erp.tenant,
            STOCK_MOVES,
            "product_id",
            &product_id,
            Some(50),
            None,
        )
        .await?;
    Ok(page.items)
}

/// Search the catalogue: one family, one word in the name, sorted.
///
/// Filters combine with AND. `Contains` is a case-insensitive substring, but
/// a term shorter than 3 characters is not indexable, and a query where no
/// filter is indexable is refused rather than served by a full scan — which
/// is why the `Eq` on `family` is there alongside it.
pub async fn search_products(erp: &Erp, family: &str, word: &str) -> Result<(Vec<Product>, u64)> {
    // These types are `#[non_exhaustive]`: they are built through `new`, not
    // a struct literal, so the SDK can add a field without breaking callers.
    let filters = [
        DocumentQueryFilter::new("family", DocumentQueryOperator::Eq, vec![json!(family)]),
        DocumentQueryFilter::new("name", DocumentQueryOperator::Contains, vec![json!(word)]),
    ];
    let sort = [DocumentQuerySort::new(
        "name",
        DocumentQuerySortDirection::Asc,
    )];

    let page = erp
        .client
        .query_documents::<Product>(&erp.tenant, PRODUCTS, &filters, &sort, Some(50), None)
        .await?;
    Ok((page.items, page.total_count))
}

/// Products to reorder.
///
/// The operators are `Eq`, `In` and `Contains` only — there is no comparison
/// between two fields, so "stock < min_stock" cannot be a filter. We ask the
/// server for active products sorted by stock and compare here, on a set it
/// has already narrowed and ordered.
pub async fn products_to_reorder(erp: &Erp) -> Result<Vec<Product>> {
    let filters = [DocumentQueryFilter::new(
        "active",
        DocumentQueryOperator::Eq,
        vec![json!(true)],
    )];
    let sort = [DocumentQuerySort::new(
        "stock",
        DocumentQuerySortDirection::Asc,
    )];

    let page = erp
        .client
        .query_documents::<Product>(&erp.tenant, PRODUCTS, &filters, &sort, Some(50), None)
        .await?;
    Ok(page
        .items
        .into_iter()
        .filter(|product| product.stock < product.min_stock)
        .collect())
}

/// Invoices still to be collected, oldest due date first.
///
/// `In` takes several values on one field where `Eq` takes one. Dates are
/// stored as ISO strings, which is what makes the server's lexicographic
/// sort match chronological order.
pub async fn unpaid_invoices(erp: &Erp) -> Result<(Vec<Invoice>, u64)> {
    let filters = [DocumentQueryFilter::new(
        "status",
        DocumentQueryOperator::In,
        vec![json!(INVOICE_ISSUED), json!(INVOICE_OVERDUE)],
    )];
    let sort = [DocumentQuerySort::new(
        "due_date",
        DocumentQuerySortDirection::Asc,
    )];

    let page = erp
        .client
        .query_documents::<Invoice>(&erp.tenant, INVOICES, &filters, &sort, Some(50), None)
        .await?;
    Ok((page.items, page.total_count))
}

/// Which collections exist for this tenant, and how many documents each holds.
pub async fn list_collections(erp: &Erp) -> Result<Vec<(String, u64)>> {
    let page = erp
        .client
        .list_collections(&erp.tenant, Some(50), None)
        .await?;
    Ok(page
        .items
        .into_iter()
        .map(|info| (info.collection, info.count))
        .collect())
}

/// Delete a document. This is idempotent: deleting an id that is not there
/// succeeds, unlike `delete_edge`.
pub async fn delete(erp: &Erp, collection: &str, id: &str) -> Result<()> {
    erp.client
        .delete_document(&erp.tenant, collection, id)
        .await?;
    Ok(())
}

/// Delete with a chosen key, so a restarted cleanup does not replay.
pub async fn delete_with_key(erp: &Erp, collection: &str, id: &str) -> Result<()> {
    erp.client
        .delete_document_with_request_id(
            &erp.tenant,
            collection,
            id,
            erp.key(&format!("delete:{collection}:{id}")),
        )
        .await?;
    Ok(())
}
