//! Conventions de nommage : collections documentaires, graphe, buckets.
//!
//! RociaDB ne connait pas le metier : ni collection, ni graphe, ni bucket ne
//! sont declares a l'avance, ils apparaissent au premier ecrit. Tout le
//! vocabulaire de l'ERP tient donc dans ce module, pour qu'un identifiant ne
//! soit jamais recompose a la main ailleurs dans le projet.

use serde::{Deserialize, Serialize};

// --- Collections documentaires ------------------------------------------

pub const CLIENTS: &str = "clients";
pub const FOURNISSEURS: &str = "fournisseurs";
pub const ARTICLES: &str = "articles";
pub const DEVIS: &str = "devis";
pub const BONS_COMMANDE: &str = "bons_commande";
pub const FACTURES: &str = "factures";
pub const MOUVEMENTS_STOCK: &str = "mouvements_stock";

/// Toutes les collections ecrites par cet exemple, dans l'ordre ou il faut
/// les supprimer (les pieces filles avant les tiers).
pub const TOUTES_COLLECTIONS: &[&str] = &[
    MOUVEMENTS_STOCK,
    FACTURES,
    BONS_COMMANDE,
    DEVIS,
    ARTICLES,
    CLIENTS,
    FOURNISSEURS,
];

// --- Labels de noeuds ----------------------------------------------------
//
// `create_document(.., node_label, node_graph)` compose l'identifiant du
// noeud en `"{label}:{document_id}"`. Les constantes ci-dessous servent donc
// aux deux bouts : a la creation du document, et a la reconstruction de
// l'identifiant de noeud pour une traversee.

pub const LABEL_CLIENT: &str = "client";
pub const LABEL_FOURNISSEUR: &str = "fournisseur";
pub const LABEL_ARTICLE: &str = "article";
pub const LABEL_DEVIS: &str = "devis";
pub const LABEL_BON_COMMANDE: &str = "bon_commande";
pub const LABEL_FACTURE: &str = "facture";

// --- Labels d'aretes -----------------------------------------------------

/// `fournisseur:F-x` -> `article:ART-y` : qui approvisionne quoi.
pub const FOURNIT: &str = "fournit";
/// `client:C-x` -> `devis:DEV-y` : a qui le devis a ete adresse.
pub const A_DEMANDE: &str = "a_demande";
/// `devis:DEV-x` -> `article:ART-y` : les articles chiffres dans le devis.
pub const PORTE_SUR: &str = "porte_sur";
/// `devis:DEV-x` -> `bon_commande:BC-y` : le devis accepte.
pub const CONVERTI_EN: &str = "converti_en";
/// `bon_commande:BC-x` -> `facture:FAC-y` : la commande facturee.
pub const FACTURE_PAR: &str = "facture_par";

/// Identifiant de noeud a partir d'un label et d'un identifiant de document.
pub fn noeud(label: &str, document_id: &str) -> String {
    format!("{label}:{document_id}")
}

/// Identifiant d'arete : `delete_edge` ne prend que cet identifiant, il doit
/// donc etre reconstructible sans relire le graphe.
pub fn arete(label: &str, de: &str, vers: &str) -> String {
    format!("{label}|{de}|{vers}")
}

/// Valeur JSON qu'ecrit `create_document` dans le noeud qu'il lie au
/// document : un simple pointeur `{collection, id}` vers la source de verite.
///
/// Le graphe est un index de navigation, jamais la source de verite : aucune
/// RPC ne permet de relire la valeur d'une arete (`neighbors_out` ne rend que
/// `node_id` + `edge_id`), donc tout ce qui doit etre relu vit dans le
/// document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RefDocument {
    pub collection: String,
    pub id: String,
}

/// Noeud enrichi ecrit par `put_node` : le pointeur `{collection, id}` plus
/// quelques champs denormalises, pour qu'une traversee affiche un resultat
/// lisible sans relire chaque document.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct NoeudArticle {
    pub collection: String,
    pub id: String,
    pub reference: String,
    pub designation: String,
    pub famille: String,
}

// --- Stockage de fichiers ------------------------------------------------

/// Bucket des pieces jointes (PDF de facture, exports).
pub const BUCKET_PIECES: &str = "pieces-jointes";

/// Identifiant de fichier du PDF d'une facture.
pub fn pdf_facture(facture_id: &str) -> String {
    format!("factures/{facture_id}.pdf")
}

/// Identifiant de fichier de l'export d'inventaire du jour.
pub fn export_stock(date: &str) -> String {
    format!("exports/stock-{date}.csv")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn l_identifiant_de_noeud_suit_la_convention_du_sdk() {
        // `create_document` compose exactement `"{label}:{id}"` : si cette
        // convention changeait, toutes les traversees viseraient a cote.
        assert_eq!(noeud(LABEL_CLIENT, "C-001"), "client:C-001");
        assert_eq!(
            noeud(LABEL_FACTURE, "FAC-2026-0001"),
            "facture:FAC-2026-0001"
        );
    }

    #[test]
    fn l_identifiant_d_arete_est_reconstructible() {
        let de = noeud(LABEL_FOURNISSEUR, "F-001");
        let vers = noeud(LABEL_ARTICLE, "ART-001");
        assert_eq!(
            arete(FOURNIT, &de, &vers),
            "fournit|fournisseur:F-001|article:ART-001"
        );
    }

    #[test]
    fn les_identifiants_de_fichier_sont_ranges_par_prefixe() {
        assert_eq!(pdf_facture("FAC-2026-0001"), "factures/FAC-2026-0001.pdf");
        assert_eq!(export_stock("2026-09-01"), "exports/stock-2026-09-01.csv");
    }
}
