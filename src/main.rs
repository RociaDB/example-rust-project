//! A small ERP — quotes, orders, invoices, stock, customers, suppliers —
//! built on the RociaDB Rust SDK.
//!
//! The business side is deliberately small. The point is to use every part
//! of the SDK once, at the place where it is the right tool.
//!
//! Run it with:
//!
//! ```text
//! ROCIA_NO_AUTH=1 cargo run
//! ```

mod documents;
mod files;
mod graph;
mod model;

use model::*;
use rocia_db_sdk::{RociaDbBuilder, RociaDbClient, RociaDbError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Errors are boxed: an example does not need its own error enum, and every
/// error type here (`RociaDbError`, `tonic::Status`, `serde_json::Error`)
/// already implements `Error`, so `?` just works.
pub type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

/// Graphs and buckets are never declared either: they exist from the first
/// node or file written to them.
pub const GRAPH: &str = "erp";
pub const BUCKET: &str = "attachments";

// Fixed dates keep the example readable and its output stable.
const TODAY: &str = "2026-09-01";
const DUE_DATE: &str = "2026-10-01";

/// The client, the tenant, and the prefix for idempotency keys.
pub struct Erp {
    pub client: RociaDbClient,
    pub tenant: String,
    run: String,
}

impl Erp {
    /// An idempotency key for one write.
    ///
    /// The server deduplicates on `(tenant, operation, request_id)` for 24
    /// hours. A *stable* key is what makes an interrupted import safe to
    /// replay — but if this demo reused the same keys on every run, a second
    /// run after a cleanup would write nothing at all: the server would see
    /// yesterday's writes replayed. So the prefix changes per run, and the
    /// key is stable within one.
    pub fn key(&self, name: &str) -> String {
        format!("{}:{name}", self.run)
    }
}

#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("\nFailed: {error}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let host = std::env::var("ROCIA_HOST").unwrap_or_else(|_| "http://127.0.0.1:50051".to_string());
    let tenant = std::env::var("ROCIA_TENANT").unwrap_or_else(|_| "demo".to_string());

    let erp = Erp {
        client: build_client(&host).await?,
        tenant,
        run: format!(
            "run-{}",
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system clock is before 1970")
                .as_secs()
        ),
    };

    println!(
        "Tenant \"{}\", graph \"{GRAPH}\", bucket \"{BUCKET}\"",
        erp.tenant
    );

    seed(&erp).await?;
    query_catalogue(&erp).await?;
    sell(&erp).await?;
    attachments(&erp).await?;
    traverse(&erp).await?;
    explore(&erp).await?;
    show_error_handling(&erp).await?;

    if std::env::var("ROCIA_CLEANUP").is_ok() {
        cleanup(&erp).await?;
    }

    println!("\nDone.");
    Ok(())
}

/// Build the client.
///
/// Authentication is on by default: without `disable_auth()`, the builder
/// reads `AUTH_TOKEN_URL`, `AUTH_CLIENT_ID` and `AUTH_CLIENT_SECRET` itself
/// at `build()` time. Set `ROCIA_NO_AUTH=1` for a local server.
async fn build_client(host: &str) -> Result<RociaDbClient> {
    // The builder mutates in place and returns `&mut Self`, so it lives in a
    // variable rather than being chained straight into `build()`.
    let mut builder = RociaDbBuilder::new();
    builder.host(host);
    builder.connect_timeout(Duration::from_secs(10));

    if std::env::var("ROCIA_NO_AUTH").is_ok() {
        builder.disable_auth();
    } else if let (Ok(url), Ok(id), Ok(secret)) = (
        std::env::var("AUTH_TOKEN_URL"),
        std::env::var("AUTH_CLIENT_ID"),
        std::env::var("AUTH_CLIENT_SECRET"),
    ) {
        // Passing them explicitly is the same thing the builder would do
        // from the environment; shown here because credentials usually come
        // from a vault rather than the process environment.
        builder.auth_client_credentials(url, id, secret);
    }

    // Cloning a `RociaDbClient` is cheap: clones share the channel, the
    // token manager and the background refresh task. Every method takes
    // `&self`, so one behind an `Arc` needs no `Mutex`.
    Ok(builder.build().await?)
}

/// Step 1: customers, suppliers, products, and who supplies what.
async fn seed(erp: &Erp) -> Result<()> {
    step("1. Customers, suppliers and catalogue");

    for customer in demo_customers() {
        documents::create_customer(erp, &customer).await?;
    }
    for supplier in demo_suppliers() {
        documents::create_supplier(erp, &supplier).await?;
    }
    println!("   3 customers and 2 suppliers written, each with its graph node");

    let products = demo_products();
    for product in &products {
        documents::import_product(erp, product).await?;
    }
    // Product nodes are written separately and enriched, so a traversal can
    // show a name without reading each document again.
    graph::put_product_nodes(erp, &products).await?;
    println!("   {} products imported", products.len());

    // A product added outside the import: `put_document` writes the
    // document, `put_node` writes its node. That is what `create_document`
    // does in one call.
    let extra = Product {
        id: "P-006".to_string(),
        reference: "GLUE-PU".to_string(),
        name: "Polyurethane glue 310 ml".to_string(),
        family: "hardware".to_string(),
        unit_price: 890,
        vat_rate: VAT_STANDARD,
        stock: 24,
        min_stock: 10,
        active: true,
    };
    documents::save_product(erp, &extra).await?;
    graph::put_node(
        erp,
        &graph::node("product", &extra.id),
        &graph::DocRef {
            collection: documents::PRODUCTS.to_string(),
            id: extra.id.clone(),
        },
    )
    .await?;
    println!("   1 product added outside the import (put_document + put_node)");

    // `create_document` writes the document and then the node, with no
    // transaction between them: if the second write fails, the document is
    // left unbound. This is the repair — same payload, stable key, so
    // running it twice costs nothing.
    graph::put_node_with_key(
        erp,
        &graph::node("customer", "C-003"),
        &graph::DocRef {
            collection: documents::CUSTOMERS.to_string(),
            id: "C-003".to_string(),
        },
    )
    .await?;
    println!("   customer C-003 node binding re-asserted (put_node_with_request_id)");

    // Edges last: both endpoints must already exist as nodes.
    graph::link_supplier_products(
        erp,
        "S-001",
        &[("P-001", 780), ("P-002", 1180), ("P-003", 190)],
    )
    .await?;
    graph::link_supplier_products(erp, "S-002", &[("P-004", 9800), ("P-001", 830)]).await?;
    println!("   \"supplies\" edges created");

    // A delivery from a supplier: one stock move in.
    let product = move_stock(erp, "P-005", "in", 50, "delivery from S-001").await?;
    println!(
        "   received 50 x {}, stock now {}",
        product.name, product.stock
    );
    Ok(())
}

/// Step 2: the four ways to read documents.
async fn query_catalogue(erp: &Erp) -> Result<()> {
    step("2. Reading the catalogue");

    let products = documents::all_products(erp).await?;
    let value: i64 = products.iter().map(|p| p.unit_price * p.stock).sum();
    println!(
        "   list_documents: {} products, stock worth {}",
        products.len(),
        money(value)
    );

    let (matches, total) = documents::search_products(erp, "hardware", "screw").await?;
    println!(
        "   query_documents (family = hardware AND name contains \"screw\"): {} of {total}",
        matches.len()
    );
    for product in &matches {
        println!("     - {} at {}", product.name, money(product.unit_price));
    }

    let to_reorder = documents::products_to_reorder(erp).await?;
    println!(
        "   below minimum stock: {}",
        join(
            to_reorder
                .iter()
                .map(|p| format!("{} ({}/{})", p.reference, p.stock, p.min_stock))
        )
    );

    let email = "orders@bertrand.example";
    let found = documents::find_customer_by_email(erp, email).await?;
    println!(
        "   search_documents on \"{email}\": {}",
        found.first().map_or("nothing", |c| c.name.as_str())
    );
    Ok(())
}

/// Step 3: quote, order, shipment, invoice, payment.
async fn sell(erp: &Erp) -> Result<()> {
    step("3. Quote, order, invoice");

    let customer: Customer = documents::get(erp, documents::CUSTOMERS, "C-001").await?;
    let screws: Product = documents::get(erp, documents::PRODUCTS, "P-001").await?;
    let brackets: Product = documents::get(erp, documents::PRODUCTS, "P-003").await?;

    let lines = vec![
        Line {
            product_id: screws.id.clone(),
            name: screws.name.clone(),
            quantity: 12,
            unit_price: screws.unit_price,
            vat_rate: screws.vat_rate,
        },
        Line {
            product_id: brackets.id.clone(),
            name: brackets.name.clone(),
            quantity: 80,
            unit_price: brackets.unit_price,
            vat_rate: brackets.vat_rate,
        },
    ];

    // The quote.
    let quote = Quote {
        id: "Q-2026-0001".to_string(),
        customer_id: customer.id.clone(),
        status: QUOTE_SENT.to_string(),
        date: TODAY.to_string(),
        totals: totals(&lines),
        lines,
    };
    documents::create_bound(erp, documents::QUOTES, "quote", &quote.id, &quote).await?;
    graph::link(
        erp,
        graph::REQUESTED,
        &graph::node("customer", &customer.id),
        &graph::node("quote", &quote.id),
    )
    .await?;
    println!(
        "   quote {} for {}: {} net, {} VAT, {} gross",
        quote.id,
        customer.name,
        money(quote.totals.net),
        money(quote.totals.vat),
        money(quote.totals.gross)
    );

    // Accepted, so it becomes an order.
    let mut accepted = quote.clone();
    accepted.status = QUOTE_ACCEPTED.to_string();
    documents::put(erp, documents::QUOTES, &accepted.id, &accepted).await?;

    let order = Order {
        id: "SO-2026-0001".to_string(),
        customer_id: customer.id.clone(),
        quote_id: quote.id.clone(),
        status: ORDER_PREPARING.to_string(),
        date: TODAY.to_string(),
        totals: quote.totals,
        lines: quote.lines.clone(),
    };
    documents::create_bound(erp, documents::ORDERS, "order", &order.id, &order).await?;
    graph::link_with_key(
        erp,
        graph::CONVERTED_TO,
        &graph::node("quote", &quote.id),
        &graph::node("order", &order.id),
    )
    .await?;
    println!("   quote accepted, order {} created", order.id);

    // Shipped: one stock move out per line.
    for line in &order.lines {
        let product = move_stock(erp, &line.product_id, "out", line.quantity, &order.id).await?;
        println!(
            "     - {}: -{} leaves {} in stock",
            product.reference, line.quantity, product.stock
        );
    }
    let mut shipped = order.clone();
    shipped.status = ORDER_SHIPPED.to_string();
    documents::put(erp, documents::ORDERS, &shipped.id, &shipped).await?;

    // Invoiced.
    let invoice = Invoice {
        id: "INV-2026-0001".to_string(),
        customer_id: customer.id.clone(),
        order_id: order.id.clone(),
        status: INVOICE_ISSUED.to_string(),
        date: TODAY.to_string(),
        due_date: DUE_DATE.to_string(),
        totals: order.totals,
        lines: order.lines.clone(),
    };
    documents::create_bound(erp, documents::INVOICES, "invoice", &invoice.id, &invoice).await?;
    graph::link_with_key(
        erp,
        graph::BILLED_AS,
        &graph::node("order", &order.id),
        &graph::node("invoice", &invoice.id),
    )
    .await?;
    println!(
        "   invoice {} issued, due {}, {} gross",
        invoice.id,
        invoice.due_date,
        money(invoice.totals.gross)
    );

    let (unpaid, total) = documents::unpaid_invoices(erp).await?;
    println!(
        "   query_documents (status In [issued, overdue], due date first): {} of {total}",
        unpaid.len()
    );

    // Paid.
    let mut paid = invoice.clone();
    paid.status = INVOICE_PAID.to_string();
    documents::put(erp, documents::INVOICES, &paid.id, &paid).await?;
    println!("   invoice {} marked {}", paid.id, paid.status);

    let moves = documents::moves_for_product(erp, "P-001").await?;
    println!(
        "   stock moves on P-001: {}",
        join(
            moves
                .iter()
                .map(|m| format!("{} {}", m.direction, m.quantity))
        )
    );

    // A second quote, declined by the customer.
    //
    // Order matters: `delete_document` is idempotent, `delete_edge` is not.
    // Removing the document first means a restart after a crash passes
    // quietly over the done half and finishes the edge; the other way round
    // it would hit NOT_FOUND.
    let drill: Product = documents::get(erp, documents::PRODUCTS, "P-004").await?;
    let declined_lines = vec![Line {
        product_id: drill.id.clone(),
        name: drill.name.clone(),
        quantity: 2,
        unit_price: drill.unit_price,
        vat_rate: drill.vat_rate,
    }];
    let declined = Quote {
        id: "Q-2026-0002".to_string(),
        customer_id: "C-002".to_string(),
        status: QUOTE_SENT.to_string(),
        date: TODAY.to_string(),
        totals: totals(&declined_lines),
        lines: declined_lines,
    };
    documents::create_bound(erp, documents::QUOTES, "quote", &declined.id, &declined).await?;
    let customer_node = graph::node("customer", &declined.customer_id);
    let quote_node = graph::node("quote", &declined.id);
    graph::link(erp, graph::REQUESTED, &customer_node, &quote_node).await?;

    documents::delete_with_key(erp, documents::QUOTES, &declined.id).await?;
    graph::unlink_with_key(
        erp,
        &graph::edge(graph::REQUESTED, &customer_node, &quote_node),
    )
    .await?;
    println!("   quote {} declined and removed", declined.id);
    Ok(())
}

/// Step 4: the three uploads and the two downloads.
async fn attachments(erp: &Erp) -> Result<()> {
    step("4. Attachments");

    // 4a. The invoice as a text document: it fits in memory, so
    //     `upload_file` handles chunking and hashing.
    let invoice: Invoice = documents::get(erp, documents::INVOICES, "INV-2026-0001").await?;
    let mut text = format!("INVOICE {}\nDue {}\n\n", invoice.id, invoice.due_date);
    for line in &invoice.lines {
        text.push_str(&format!(
            "{} x {} = {}\n",
            line.quantity,
            line.name,
            money(line.unit_price * line.quantity)
        ));
    }
    text.push_str(&format!("\nTotal: {}\n", money(invoice.totals.gross)));

    let invoice_file = format!("invoices/{}.txt", invoice.id);
    files::upload(erp, &invoice_file, text.as_bytes(), "text/plain").await?;
    println!("   upload_file: {invoice_file} ({} bytes)", text.len());

    // 4b. The stock export: produced in pieces, re-chunked by the SDK.
    let products = documents::all_products(erp).await?;
    let mut csv = String::from("reference;name;family;stock;min_stock;unit_price\n");
    for product in &products {
        csv.push_str(&format!(
            "{};{};{};{};{};{}\n",
            product.reference,
            product.name,
            product.family,
            product.stock,
            product.min_stock,
            product.unit_price
        ));
    }
    let export_file = format!("exports/stock-{TODAY}.csv");
    files::upload_streamed(erp, &export_file, csv.into_bytes(), "text/csv").await?;
    println!("   upload_file_chunked: {export_file}");

    // 4c. A short note, one hand-built message.
    files::upload_raw(
        erp,
        "notes/reorder.txt",
        b"Check P-005 before the next order.\n",
    )
    .await?;
    println!("   upload_file_stream: notes/reorder.txt");

    let info = files::stat(erp, &invoice_file).await?;
    println!(
        "   stat_file: {} bytes, {}, created {}",
        info.size_bytes, info.content_type, info.created_at
    );

    let downloaded = files::download(erp, &invoice_file).await?;
    println!(
        "   download_file: {} bytes back, identical: {}",
        downloaded.len(),
        downloaded == text.as_bytes()
    );

    let streamed = files::download_streamed(erp, &export_file).await?;
    println!("   download_file_stream: {streamed} bytes read");

    println!(
        "   list_files: {}",
        join(files::list_files(erp).await?.into_iter())
    );

    files::delete_with_key(erp, "notes/reorder.txt").await?;
    println!("   delete_file: note removed");
    Ok(())
}

/// Step 5: graph traversals.
async fn traverse(erp: &Erp) -> Result<()> {
    step("5. Graph traversals");

    let supplied = graph::products_of_supplier(erp, "S-001").await?;
    println!(
        "   get_outgoing_neighbor_nodes (S-001 -supplies->): {}",
        join(supplied.iter().map(|p| p.name.clone()))
    );

    let sources = graph::suppliers_of_product(erp, "P-001").await?;
    println!(
        "   get_incoming_neighbor_nodes (-supplies-> P-001): {}",
        join(sources.into_iter())
    );

    let quotes =
        graph::neighbors_out(erp, &graph::node("customer", "C-001"), graph::REQUESTED).await?;
    println!(
        "   neighbors_out (C-001 -requested->): {}",
        join(quotes.iter().map(|n| n.node_id.clone()))
    );

    let orders = graph::neighbors_in(
        erp,
        &graph::node("invoice", "INV-2026-0001"),
        graph::BILLED_AS,
    )
    .await?;
    println!(
        "   neighbors_in (-billed_as-> INV-2026-0001): {}",
        join(orders.iter().map(|n| n.node_id.clone()))
    );

    // The node `create_document` wrote: a pointer to the document.
    let node_id = graph::node("invoice", "INV-2026-0001");
    println!(
        "   get_node (raw): {}",
        graph::raw_node(erp, &node_id).await?
    );
    let doc_ref = graph::node_ref(erp, &node_id).await?;
    println!(
        "   get_node_as (typed): collection \"{}\", id \"{}\"",
        doc_ref.collection, doc_ref.id
    );
    Ok(())
}

/// Step 6: what the deployment holds, and the token.
async fn explore(erp: &Erp) -> Result<()> {
    step("6. Exploring the deployment");

    // `list_tenants` is the one RPC not scoped to a tenant. It enumerates the
    // whole deployment and may be refused by a dedicated policy, so
    // PERMISSION_DENIED here means "not your role", not "broken".
    //
    // Worth knowing: `tenant_id` is a business partition, not a security
    // boundary. It is derived from no identity — any authenticated client can
    // address any tenant. Enforcing who may touch what is the application's job.
    match erp.client.list_tenants(Some(50), None).await {
        Ok(page) => println!("   list_tenants: {}", join(page.items.into_iter())),
        Err(error) if error.is_permission_denied() => {
            println!("   list_tenants: refused, this RPC covers the whole deployment");
        }
        Err(error) => return Err(error.into()),
    }

    let collections = documents::list_collections(erp).await?;
    println!(
        "   list_collections: {}",
        join(
            collections
                .iter()
                .map(|(name, count)| format!("{name} ({count})"))
        )
    );
    println!(
        "   list_graphs: {}",
        join(graph::list_graphs(erp).await?.into_iter())
    );
    let nodes = graph::list_nodes(erp).await?;
    println!("   list_nodes: {} nodes", nodes.len());
    println!(
        "   list_buckets: {}",
        join(files::list_buckets(erp).await?.into_iter())
    );
    println!(
        "   customers (free total_count): {}",
        documents::count_customers(erp).await?
    );

    // Both token calls are no-ops when the client was built with
    // `disable_auth()`, so callers need not know how it was built.
    erp.client.refresh_auth_token().await?;
    println!("   refresh_auth_token: renewed now, caller waits");
    erp.client.invalidate_auth_token();
    println!("   invalidate_auth_token: marks it stale without waiting");

    if let (Ok(url), Ok(id), Ok(secret)) = (
        std::env::var("AUTH_TOKEN_URL"),
        std::env::var("AUTH_CLIENT_ID"),
        std::env::var("AUTH_CLIENT_SECRET"),
    ) {
        auth_module_demo(&url, &id, &secret).await?;
    }
    Ok(())
}

/// The `auth` module used without a `RociaDbClient`.
///
/// `build()` sets all of this up for you. The module is public for when the
/// same token has to be used elsewhere, next to a service of your own.
async fn auth_module_demo(token_url: &str, client_id: &str, client_secret: &str) -> Result<()> {
    use rocia_db_sdk::auth::{ApiKeyInterceptor, TokenManager, fetch_token};

    let http = reqwest::Client::new();

    // One token, once. No caching, no renewal.
    let token = fetch_token(&http, token_url, client_id, client_secret).await?;

    // The manager fetches its own first token on construction.
    let manager = TokenManager::new(
        http,
        token_url.to_string(),
        client_id.to_string(),
        client_secret.to_string(),
    )
    .await?;

    // A safe interval: max(expires_in * 2/3, 5s), so it renews with a third
    // of the lifetime still left.
    let interval = manager.refresh_interval();

    // The guard is `#[must_use]` for a reason: dropping it stops the
    // background refresh immediately.
    let _guard = manager.spawn_refresh(interval);

    // The interceptor that injects the header — the same one the builder
    // installs on all four services.
    let _interceptor = manager.interceptor();

    manager.refresh_now().await?;
    manager.request_refresh();

    // Server-side, this one validates an incoming `x-api-key`. It has no
    // business in a client; it is here because the module covers both ends.
    let _server_side = ApiKeyInterceptor::new("some-service-key".to_string());

    println!(
        "   auth module: {} token, valid {}s, refreshing every {:?}",
        token.token_type, token.expires_in, interval
    );
    Ok(())
}

/// Step 7: what a `RociaDbError` tells you.
///
/// It is a typed enum, not a boxed `dyn Error`, so you match on it instead of
/// downcasting. Two questions decide whether to retry, and they are the ones
/// to ask first.
async fn show_error_handling(erp: &Erp) -> Result<()> {
    step("7. Reading an error");

    let missing = erp
        .client
        .get_document::<Customer>(&erp.tenant, documents::CUSTOMERS, "C-DOES-NOT-EXIST")
        .await;

    match missing {
        Ok(_) => println!("   unexpectedly found a customer that should not exist"),
        Err(error) => {
            // `is_unauthenticated()` is the only case worth retrying:
            // refresh the token, then replay. `is_permission_denied()` is
            // final — the token is valid but lacks the scope, so refreshing
            // changes nothing.
            println!("   is_unauthenticated: {}", error.is_unauthenticated());
            println!("   is_permission_denied: {}", error.is_permission_denied());

            // Three accessors, coarse to fine: the gRPC code, the server's
            // own reason (`not_found`, `invalid_argument`, ...), and the raw
            // status, so nothing is lost against calling the generated
            // client directly.
            if let Some(code) = error.code() {
                println!("   code: {}", code.description());
            }
            println!("   reason: {}", error.reason().unwrap_or("(none)"));
            println!("   message: {}", error.status().map_or("", |s| s.message()));
            println!("   what to do: {}", advice(&error));
        }
    }
    Ok(())
}

/// One line of guidance per error variant.
fn advice(error: &RociaDbError) -> &'static str {
    match error {
        RociaDbError::Status { .. } => "the server refused the call; read reason() to know why",
        RociaDbError::Connection { .. } => {
            "check the server is up, and that the host carries no path"
        }
        RociaDbError::Auth { .. } => "check AUTH_TOKEN_URL / AUTH_CLIENT_ID / AUTH_CLIENT_SECRET",
        RociaDbError::Encode { .. } => "the value could not be serialized; fix the model",
        RociaDbError::Decode { .. } => "the stored document no longer matches the Rust type",
        RociaDbError::Validation(_) => "rejected client-side; nothing was sent",
    }
}

/// Step 8: remove the demo data.
///
/// Three delete semantics meet here: `delete_document` and `delete_file` are
/// idempotent, `delete_edge` is not, and **nothing deletes a node** — there
/// is no RPC for it, so a node with no edges and no document stays listed by
/// `list_nodes`.
async fn cleanup(erp: &Erp) -> Result<()> {
    step("8. Cleanup");

    // Edges first, while the traversals still find them. `Neighbor` carries
    // the real edge id, so there is nothing to rebuild.
    let mut edges = 0;
    for node_id in graph::list_nodes(erp).await? {
        for label in [
            graph::SUPPLIES,
            graph::REQUESTED,
            graph::CONVERTED_TO,
            graph::BILLED_AS,
        ] {
            for neighbor in graph::neighbors_out(erp, &node_id, label).await? {
                graph::unlink(erp, &neighbor.edge_id).await?;
                edges += 1;
            }
        }
    }

    let mut docs = 0;
    for collection in [
        documents::STOCK_MOVES,
        documents::INVOICES,
        documents::ORDERS,
        documents::QUOTES,
        documents::PRODUCTS,
        documents::CUSTOMERS,
        documents::SUPPLIERS,
    ] {
        let page = erp
            .client
            .list_documents::<serde_json::Value>(&erp.tenant, collection, Some(200), None)
            .await?;
        for document in page.items {
            if let Some(id) = document.get("id").and_then(|v| v.as_str()) {
                documents::delete(erp, collection, id).await?;
                docs += 1;
            }
        }
    }

    let mut removed = 0;
    for file_id in files::list_files(erp).await? {
        files::delete(erp, &file_id).await?;
        removed += 1;
    }

    println!("   {docs} documents, {edges} edges and {removed} files deleted");
    println!(
        "   {} nodes remain: no RPC deletes a node",
        graph::list_nodes(erp).await?.len()
    );
    Ok(())
}

/// Move stock and record the move.
///
/// Two writes, not a transaction: RociaDB offers no atomicity across
/// documents. The stock is updated first, so a crash in between leaves an
/// up-to-date stock with no trace rather than a trace with no effect — the
/// easier of the two to reconcile.
async fn move_stock(
    erp: &Erp,
    product_id: &str,
    direction: &str,
    quantity: i64,
    source: &str,
) -> Result<Product> {
    let mut product: Product = documents::get(erp, documents::PRODUCTS, product_id).await?;
    let delta = if direction == "in" {
        quantity
    } else {
        -quantity
    };
    if product.stock + delta < 0 {
        return Err(format!("not enough stock on {product_id}").into());
    }
    product.stock += delta;
    documents::save_product(erp, &product).await?;

    let stock_move = StockMove {
        id: format!("MOV-{product_id}-{direction}-{}", erp.run),
        product_id: product_id.to_string(),
        direction: direction.to_string(),
        quantity,
        source: source.to_string(),
    };
    documents::put(erp, documents::STOCK_MOVES, &stock_move.id, &stock_move).await?;
    Ok(product)
}

fn step(title: &str) {
    println!("\n{title}");
}

fn join(values: impl Iterator<Item = String>) -> String {
    let joined: Vec<String> = values.collect();
    if joined.is_empty() {
        "none".to_string()
    } else {
        joined.join(", ")
    }
}

fn demo_customers() -> Vec<Customer> {
    vec![
        Customer {
            id: "C-001".to_string(),
            name: "Bertrand Joinery".to_string(),
            email: "orders@bertrand.example".to_string(),
            city: "Nantes".to_string(),
            active: true,
        },
        Customer {
            id: "C-002".to_string(),
            name: "Woodcraft Studio".to_string(),
            email: "buying@woodcraft.example".to_string(),
            city: "Rennes".to_string(),
            active: true,
        },
        Customer {
            id: "C-003".to_string(),
            name: "Morel Framing".to_string(),
            email: "accounts@morel.example".to_string(),
            city: "Angers".to_string(),
            active: false,
        },
    ]
}

fn demo_suppliers() -> Vec<Supplier> {
    vec![
        Supplier {
            id: "S-001".to_string(),
            name: "Central Fasteners".to_string(),
            email: "sales@fasteners.example".to_string(),
            lead_time_days: 5,
        },
        Supplier {
            id: "S-002".to_string(),
            name: "ProTools".to_string(),
            email: "sales@protools.example".to_string(),
            lead_time_days: 12,
        },
    ]
}

fn demo_products() -> Vec<Product> {
    vec![
        Product {
            id: "P-001".to_string(),
            reference: "SCR-4X30".to_string(),
            name: "Wood screw 4x30 (box of 200)".to_string(),
            family: "hardware".to_string(),
            unit_price: 1250,
            vat_rate: VAT_STANDARD,
            stock: 120,
            min_stock: 40,
            active: true,
        },
        Product {
            id: "P-002".to_string(),
            reference: "SCR-5X50".to_string(),
            name: "Wood screw 5x50 (box of 100)".to_string(),
            family: "hardware".to_string(),
            unit_price: 1890,
            vat_rate: VAT_STANDARD,
            stock: 18,
            min_stock: 30,
            active: true,
        },
        Product {
            id: "P-003".to_string(),
            reference: "BRK-RAFT".to_string(),
            name: "Galvanised rafter bracket".to_string(),
            family: "hardware".to_string(),
            unit_price: 340,
            vat_rate: VAT_STANDARD,
            stock: 640,
            min_stock: 150,
            active: true,
        },
        Product {
            id: "P-004".to_string(),
            reference: "DRL-18V".to_string(),
            name: "Cordless drill 18V".to_string(),
            family: "tools".to_string(),
            unit_price: 14900,
            vat_rate: VAT_STANDARD,
            stock: 7,
            min_stock: 4,
            active: true,
        },
        Product {
            id: "P-005".to_string(),
            reference: "DOC-FIT".to_string(),
            name: "Printed fitting guide".to_string(),
            family: "documentation".to_string(),
            unit_price: 450,
            vat_rate: VAT_REDUCED,
            stock: 2,
            min_stock: 25,
            active: true,
        },
    ]
}
