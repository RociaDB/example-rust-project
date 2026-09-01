# erp-example

A small ERP — **quotes, orders, invoices, stock, customers, suppliers** —
built on [`rociadb-sdk`](https://crates.io/crates/rociadb-sdk), the RociaDB
Rust SDK.

The business logic is deliberately plain. The point is to use every part of
the SDK once, at the place where it is the right tool.

## Running it

```bash
# protoc is required: the SDK's build.rs compiles the bundled .proto files.
sudo apt-get install -y protobuf-compiler     # or: brew install protobuf

ROCIA_NO_AUTH=1 cargo run
```

With no RociaDB listening on `127.0.0.1:50051` it stops on a clear message
rather than a panic.

| Variable | Default | Meaning |
|---|---|---|
| `ROCIA_HOST` | `http://127.0.0.1:50051` | host and port, **no path** |
| `ROCIA_TENANT` | `demo` | business partition to write to |
| `ROCIA_NO_AUTH` | unset | set it to skip authentication (local dev) |
| `ROCIA_CLEANUP` | unset | set it to delete the demo data at the end |
| `AUTH_TOKEN_URL`, `AUTH_CLIENT_ID`, `AUTH_CLIENT_SECRET` | — | OAuth2 credentials |

Leave `ROCIA_NO_AUTH` unset and the SDK reads the three `AUTH_*` variables
itself — that is the builder's default behaviour. Set all three and the
example also runs a short tour of the `auth` module on its own.

## What it does

```
customer ──requested──▶ quote ──converted_to──▶ order ──billed_as──▶ invoice
                                                                  
supplier ──supplies──▶ product
```

Each business record is a **document** (the source of truth: lines, totals,
status) plus a **graph node** (an index for navigation). Shipments and
deliveries write `stock_moves` documents and update the product's stock; the
invoice text and the stock export go to the **file service**.

The program runs eight steps in order, printing what each SDK call returned:
seed, catalogue queries, the sales flow, attachments, graph traversals,
deployment exploration, error handling, and optional cleanup.

## Layout

Five files, one per SDK service area:

| File | What it covers |
|---|---|
| [`model.rs`](src/model.rs) | business types, VAT and totals, the only logic testable without a server |
| [`documents.rs`](src/documents.rs) | write, read, search, query, delete documents |
| [`graph.rs`](src/graph.rs) | nodes and edges, single and batched, traversals both ways |
| [`files.rs`](src/files.rs) | the three uploads, the two downloads, the wire contract |
| [`main.rs`](src/main.rs) | the builder, tenants, token lifecycle, error handling, the demo |

Money is stored in cents and VAT rates in basis points, so no rounding
depends on the order of operations.

## Things worth knowing about RociaDB

**Nothing is declared.** A collection, a graph or a bucket exists from the
first write to it. That is convenient, and it is why names live in constants:
a typo does not fail, it silently creates one more collection.

**The document is the source of truth; the graph is an index.** No RPC reads
an edge's value back — `neighbors_out` returns a `node_id` and an `edge_id`,
nothing else. Product nodes are written *enriched* so a traversal can show a
name without another `get_document`.

**No two writes are atomic.** `create_document` writes the document and then
the node with no transaction between them: if the second fails, the document
is left unbound, which is why the example shows the repair. Ordering is a
choice every time — `move_stock` updates the stock *before* writing its
trace, because an up-to-date stock with no trace is easier to reconcile than
the reverse.

**Deletes do not all behave the same.** `delete_document` and `delete_file`
are idempotent; `delete_edge` returns `NOT_FOUND` on a missing edge; and
**nothing deletes a node** — there is no RPC for it, so an orphaned node stays
listed by `list_nodes`.

**Idempotency keys are a design choice.** The server deduplicates on
`(tenant, operation, request_id)` for 24 hours. A stable key is what makes an
interrupted import safe to replay — but if this demo reused the same keys
every run, a second run after a cleanup would write nothing at all. So the
prefix changes per run and the key is stable within one.

**`tenant_id` is a business partition, not a security boundary.** It is
derived from no identity: any authenticated client can address any tenant.
Deciding who may touch what is the application's job.

**Only one error is worth retrying.** `UNAUTHENTICATED` is temporary (refresh
the token, replay); `PERMISSION_DENIED` is final (the token is valid but
lacks the scope). Everything else is in `reason()`.

**Most public types are `#[non_exhaustive]`.** `DocumentQueryFilter`,
`DocumentQuerySort`, `NodeInput` and `EdgeInput` are built through
`::new(..)` (plus `.with_request_id(..)` on the last two) rather than a
struct literal, and a `match` on `RociaDbError` needs a wildcard arm. That is
what lets the SDK add a field or a variant in a minor release without
breaking callers.

## Development

```bash
PROTOC=/usr/bin/protoc cargo fmt --all -- --check
PROTOC=/usr/bin/protoc cargo clippy --all-targets --all-features -- -D warnings
PROTOC=/usr/bin/protoc cargo test
```

The six unit tests are deterministic and need no server: VAT and totals, and
the node/edge id conventions. The demo itself needs a running RociaDB.

## Licence

Apache-2.0, like the SDK.
