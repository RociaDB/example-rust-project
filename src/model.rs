//! Business types, and the small amount of arithmetic that needs no server.

use serde::{Deserialize, Serialize};

/// Money is stored in cents, so no rounding depends on operation order.
pub type Cents = i64;

/// VAT rates in basis points: 2000 is 20.00%.
pub const VAT_STANDARD: i64 = 2000;
pub const VAT_REDUCED: i64 = 1000;

// Statuses are plain strings. They are written into documents and used as-is
// in query filters, so one constant per value keeps both sides in sync: a
// typo in a filter returns zero rows instead of failing.
pub const QUOTE_SENT: &str = "sent";
pub const QUOTE_ACCEPTED: &str = "accepted";
pub const ORDER_PREPARING: &str = "preparing";
pub const ORDER_SHIPPED: &str = "shipped";
pub const INVOICE_ISSUED: &str = "issued";
pub const INVOICE_OVERDUE: &str = "overdue";
pub const INVOICE_PAID: &str = "paid";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Customer {
    pub id: String,
    pub name: String,
    pub email: String,
    pub city: String,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Supplier {
    pub id: String,
    pub name: String,
    pub email: String,
    pub lead_time_days: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Product {
    pub id: String,
    pub reference: String,
    pub name: String,
    pub family: String,
    pub unit_price: Cents,
    pub vat_rate: i64,
    pub stock: i64,
    pub min_stock: i64,
    pub active: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Line {
    pub product_id: String,
    pub name: String,
    pub quantity: i64,
    pub unit_price: Cents,
    pub vat_rate: i64,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Totals {
    pub net: Cents,
    pub vat: Cents,
    pub gross: Cents,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Quote {
    pub id: String,
    pub customer_id: String,
    pub status: String,
    pub date: String,
    pub lines: Vec<Line>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    pub id: String,
    pub customer_id: String,
    pub quote_id: String,
    pub status: String,
    pub date: String,
    pub lines: Vec<Line>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Invoice {
    pub id: String,
    pub customer_id: String,
    pub order_id: String,
    pub status: String,
    pub date: String,
    pub due_date: String,
    pub lines: Vec<Line>,
    pub totals: Totals,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StockMove {
    pub id: String,
    pub product_id: String,
    /// `"in"` on a delivery from a supplier, `"out"` on a shipment.
    pub direction: String,
    pub quantity: i64,
    pub source: String,
}

/// VAT on a net amount, rounded to the nearest cent.
pub fn vat(net: Cents, rate: i64) -> Cents {
    (net * rate + 5_000) / 10_000
}

/// Add up lines. VAT is computed per line and then summed, the way it is
/// printed on the invoice: rounding once at the end would be off by a cent
/// against the printed detail.
pub fn totals(lines: &[Line]) -> Totals {
    let net: Cents = lines.iter().map(|l| l.unit_price * l.quantity).sum();
    let vat_total: Cents = lines
        .iter()
        .map(|l| vat(l.unit_price * l.quantity, l.vat_rate))
        .sum();
    Totals {
        net,
        vat: vat_total,
        gross: net + vat_total,
    }
}

/// `123456` becomes `"1234.56 EUR"`.
pub fn money(amount: Cents) -> String {
    let sign = if amount < 0 { "-" } else { "" };
    let abs = amount.abs();
    format!("{sign}{}.{:02} EUR", abs / 100, abs % 100)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(quantity: i64, unit_price: Cents, vat_rate: i64) -> Line {
        Line {
            product_id: "P-1".to_string(),
            name: "Product".to_string(),
            quantity,
            unit_price,
            vat_rate,
        }
    }

    #[test]
    fn vat_rounds_to_the_nearest_cent() {
        // 9.99 at 20% is 1.998: rounds up.
        assert_eq!(vat(999, VAT_STANDARD), 200);
        // 0.01 at 10% is 0.001: rounds down to nothing.
        assert_eq!(vat(1, VAT_REDUCED), 0);
        // Exactly half a cent rounds up.
        assert_eq!(vat(5, VAT_REDUCED), 1);
        assert_eq!(vat(0, VAT_STANDARD), 0);
    }

    #[test]
    fn totals_sum_vat_line_by_line() {
        // 3 x 9.99 = 29.97 net, 5.99 VAT; 2 x 45.50 = 91.00 net, 9.10 VAT.
        let result = totals(&[line(3, 999, VAT_STANDARD), line(2, 4550, VAT_REDUCED)]);
        assert_eq!(result.net, 2997 + 9100);
        assert_eq!(result.vat, 599 + 910);
        assert_eq!(result.gross, result.net + result.vat);
    }

    #[test]
    fn totals_of_nothing_are_zero() {
        assert_eq!(totals(&[]), Totals::default());
    }

    #[test]
    fn money_is_readable() {
        assert_eq!(money(123_456), "1234.56 EUR");
        assert_eq!(money(5), "0.05 EUR");
        assert_eq!(money(0), "0.00 EUR");
        assert_eq!(money(-999), "-9.99 EUR");
    }
}
