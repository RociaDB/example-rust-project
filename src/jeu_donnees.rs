//! Le jeu de donnees de la demonstration : une quincaillerie.

use crate::modele::{Article, Client, Fournisseur, TVA_NORMALE, TVA_REDUITE};

pub const DEVIS_ID: &str = "DEV-2026-0001";
pub const COMMANDE_ID: &str = "BC-2026-0001";
pub const FACTURE_ID: &str = "FAC-2026-0001";

pub fn clients() -> Vec<Client> {
    vec![
        Client {
            id: "C-001".to_string(),
            raison_sociale: "Menuiserie Bertrand".to_string(),
            email: "contact@menuiserie-bertrand.example".to_string(),
            siret: "40483304800022".to_string(),
            ville: "Nantes".to_string(),
            actif: true,
        },
        Client {
            id: "C-002".to_string(),
            raison_sociale: "Atelier du Bois".to_string(),
            email: "achats@atelier-du-bois.example".to_string(),
            siret: "51234567800017".to_string(),
            ville: "Rennes".to_string(),
            actif: true,
        },
        Client {
            id: "C-003".to_string(),
            raison_sociale: "Charpentes Morel".to_string(),
            email: "compta@charpentes-morel.example".to_string(),
            siret: "39876543200029".to_string(),
            ville: "Angers".to_string(),
            actif: false,
        },
    ]
}

pub fn fournisseurs() -> Vec<Fournisseur> {
    vec![
        Fournisseur {
            id: "F-001".to_string(),
            raison_sociale: "Visserie Centrale".to_string(),
            email: "commandes@visserie-centrale.example".to_string(),
            siret: "77889900100013".to_string(),
            delai_jours: 5,
            actif: true,
        },
        Fournisseur {
            id: "F-002".to_string(),
            raison_sociale: "Outillage Pro".to_string(),
            email: "adv@outillage-pro.example".to_string(),
            siret: "66554433200018".to_string(),
            delai_jours: 12,
            actif: true,
        },
    ]
}

pub fn articles() -> Vec<Article> {
    vec![
        Article {
            id: "ART-001".to_string(),
            reference: "VIS-4X30".to_string(),
            designation: "Vis a bois 4x30 (boite de 200)".to_string(),
            famille: "quincaillerie".to_string(),
            prix_unitaire_ht: 1250,
            taux_tva: TVA_NORMALE,
            stock: 120,
            stock_mini: 40,
            actif: true,
        },
        Article {
            id: "ART-002".to_string(),
            reference: "VIS-5X50".to_string(),
            designation: "Vis a bois 5x50 (boite de 100)".to_string(),
            famille: "quincaillerie".to_string(),
            prix_unitaire_ht: 1890,
            taux_tva: TVA_NORMALE,
            stock: 18,
            stock_mini: 30,
            actif: true,
        },
        Article {
            id: "ART-003".to_string(),
            reference: "EQU-CHEV".to_string(),
            designation: "Equerre de chevron galvanisee".to_string(),
            famille: "quincaillerie".to_string(),
            prix_unitaire_ht: 340,
            taux_tva: TVA_NORMALE,
            stock: 640,
            stock_mini: 150,
            actif: true,
        },
        Article {
            id: "ART-004".to_string(),
            reference: "PERC-18V".to_string(),
            designation: "Perceuse visseuse 18V".to_string(),
            famille: "outillage".to_string(),
            prix_unitaire_ht: 14900,
            taux_tva: TVA_NORMALE,
            stock: 7,
            stock_mini: 4,
            actif: true,
        },
        Article {
            id: "ART-005".to_string(),
            reference: "DOC-POSE".to_string(),
            designation: "Notice de pose imprimee".to_string(),
            famille: "documentation".to_string(),
            prix_unitaire_ht: 450,
            taux_tva: TVA_REDUITE,
            stock: 2,
            stock_mini: 25,
            actif: true,
        },
    ]
}

/// Qui approvisionne quoi, et a quel prix d'achat (en centimes).
pub fn approvisionnements() -> Vec<(&'static str, Vec<(String, i64)>)> {
    vec![
        (
            "F-001",
            vec![
                ("ART-001".to_string(), 780),
                ("ART-002".to_string(), 1180),
                ("ART-003".to_string(), 190),
            ],
        ),
        (
            "F-002",
            vec![("ART-004".to_string(), 9800), ("ART-001".to_string(), 830)],
        ),
    ]
}
