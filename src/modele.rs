//! Le modele metier de l'ERP, et les seuls calculs qui n'ont pas besoin de
//! serveur : montants, TVA, valorisation du stock.
//!
//! Les montants sont des entiers de centimes (`Centimes`) et les taux de TVA
//! des points de base (`2000` = 20,00 %) : aucun flottant ne circule, donc
//! aucun arrondi ne depend de l'ordre des operations.

use serde::{Deserialize, Serialize};

/// Un montant, en centimes d'euro.
pub type Centimes = i64;

/// Un taux, en points de base : `2000` vaut 20,00 %.
pub type PointsDeBase = u32;

pub const TVA_NORMALE: PointsDeBase = 2000;
pub const TVA_REDUITE: PointsDeBase = 1000;

/// Formatte un montant a la francaise : `123456` -> `"1234,56 EUR"`.
pub fn formater(montant: Centimes) -> String {
    let signe = if montant < 0 { "-" } else { "" };
    let absolu = montant.abs();
    format!("{signe}{},{:02} EUR", absolu / 100, absolu % 100)
}

/// TVA sur un montant hors taxes, arrondie au centime le plus proche.
pub fn tva(montant_ht: Centimes, taux: PointsDeBase) -> Centimes {
    let produit = montant_ht * i64::from(taux);
    if produit >= 0 {
        (produit + 5_000) / 10_000
    } else {
        (produit - 5_000) / 10_000
    }
}

// --- Tiers ---------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Client {
    pub id: String,
    pub raison_sociale: String,
    pub email: String,
    pub siret: String,
    pub ville: String,
    pub actif: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Fournisseur {
    pub id: String,
    pub raison_sociale: String,
    pub email: String,
    pub siret: String,
    /// Delai d'approvisionnement annonce, en jours.
    pub delai_jours: u32,
    pub actif: bool,
}

// --- Catalogue et stock --------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Article {
    pub id: String,
    pub reference: String,
    pub designation: String,
    pub famille: String,
    pub prix_unitaire_ht: Centimes,
    pub taux_tva: PointsDeBase,
    pub stock: i64,
    /// Seuil de reapprovisionnement.
    pub stock_mini: i64,
    pub actif: bool,
}

impl Article {
    /// Vrai quand le stock est passe sous le seuil de reapprovisionnement.
    pub fn sous_le_seuil(&self) -> bool {
        self.stock < self.stock_mini
    }

    /// Valeur du stock au prix d'achat theorique (ici, le prix de vente HT).
    pub fn valeur_stock(&self) -> Centimes {
        self.prix_unitaire_ht * self.stock
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SensMouvement {
    Entree,
    Sortie,
}

impl SensMouvement {
    pub fn code(self) -> &'static str {
        match self {
            Self::Entree => "entree",
            Self::Sortie => "sortie",
        }
    }

    /// Signe applique au stock : `+1` a la reception, `-1` a l'expedition.
    pub fn signe(self) -> i64 {
        match self {
            Self::Entree => 1,
            Self::Sortie => -1,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct MouvementStock {
    pub id: String,
    pub article_id: String,
    pub sens: SensMouvement,
    pub quantite: i64,
    /// Piece a l'origine du mouvement : `"BC-2026-0001"`, `"reception F-001"`.
    pub origine: String,
    pub horodatage: String,
}

// --- Pieces commerciales -------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Ligne {
    pub article_id: String,
    pub designation: String,
    pub quantite: i64,
    pub prix_unitaire_ht: Centimes,
    pub taux_tva: PointsDeBase,
}

impl Ligne {
    pub fn total_ht(&self) -> Centimes {
        self.prix_unitaire_ht * self.quantite
    }

    pub fn total_tva(&self) -> Centimes {
        tva(self.total_ht(), self.taux_tva)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Totaux {
    pub total_ht: Centimes,
    pub total_tva: Centimes,
    pub total_ttc: Centimes,
}

/// Totalise des lignes. La TVA est calculee ligne a ligne puis sommee, comme
/// sur une facture papier : arrondir une seule fois a la fin donnerait un
/// centime d'ecart avec le detail imprime.
pub fn totaliser(lignes: &[Ligne]) -> Totaux {
    let total_ht = lignes.iter().map(Ligne::total_ht).sum();
    let total_tva = lignes.iter().map(Ligne::total_tva).sum();
    Totaux {
        total_ht,
        total_tva,
        total_ttc: total_ht + total_tva,
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatutDevis {
    Brouillon,
    Envoye,
    Accepte,
    Refuse,
}

impl StatutDevis {
    /// La meme chaine que celle produite par serde : c'est elle qui part dans
    /// un `DocumentQueryFilter`, ou une faute de frappe ne renverrait aucun
    /// resultat sans jamais lever d'erreur.
    pub fn code(self) -> &'static str {
        match self {
            Self::Brouillon => "brouillon",
            Self::Envoye => "envoye",
            Self::Accepte => "accepte",
            Self::Refuse => "refuse",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatutCommande {
    EnPreparation,
    Expediee,
    Livree,
    Annulee,
}

impl StatutCommande {
    pub fn code(self) -> &'static str {
        match self {
            Self::EnPreparation => "en_preparation",
            Self::Expediee => "expediee",
            Self::Livree => "livree",
            Self::Annulee => "annulee",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum StatutFacture {
    Emise,
    Payee,
    EnRetard,
    Annulee,
}

impl StatutFacture {
    pub fn code(self) -> &'static str {
        match self {
            Self::Emise => "emise",
            Self::Payee => "payee",
            Self::EnRetard => "en_retard",
            Self::Annulee => "annulee",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Devis {
    pub id: String,
    pub client_id: String,
    pub statut: StatutDevis,
    pub date: String,
    pub validite: String,
    pub lignes: Vec<Ligne>,
    pub totaux: Totaux,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct BonCommande {
    pub id: String,
    pub client_id: String,
    /// Le devis a l'origine de la commande, quand il y en a un.
    pub devis_id: Option<String>,
    pub statut: StatutCommande,
    pub date: String,
    pub lignes: Vec<Ligne>,
    pub totaux: Totaux,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Facture {
    pub id: String,
    pub client_id: String,
    pub bon_commande_id: String,
    pub statut: StatutFacture,
    pub date: String,
    pub date_echeance: String,
    pub lignes: Vec<Ligne>,
    pub totaux: Totaux,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ligne(quantite: i64, prix: Centimes, taux: PointsDeBase) -> Ligne {
        Ligne {
            article_id: "ART-001".to_string(),
            designation: "Article".to_string(),
            quantite,
            prix_unitaire_ht: prix,
            taux_tva: taux,
        }
    }

    #[test]
    fn la_tva_est_arrondie_au_centime_le_plus_proche() {
        // 9,99 EUR a 20 % font 1,998 EUR : le centime superieur.
        assert_eq!(tva(999, TVA_NORMALE), 200);
        // 0,01 EUR a 10 % font 0,001 EUR : rien.
        assert_eq!(tva(1, TVA_REDUITE), 0);
        // 0,05 EUR a 10 % font 0,005 EUR : le centime superieur (arrondi au
        // superieur sur une valeur pile a la moitie).
        assert_eq!(tva(5, TVA_REDUITE), 1);
        assert_eq!(tva(0, TVA_NORMALE), 0);
    }

    #[test]
    fn la_tva_d_un_avoir_s_arrondit_symetriquement() {
        assert_eq!(tva(-999, TVA_NORMALE), -200);
        assert_eq!(tva(-5, TVA_REDUITE), -1);
    }

    #[test]
    fn les_totaux_somment_la_tva_ligne_a_ligne() {
        let lignes = vec![ligne(3, 999, TVA_NORMALE), ligne(2, 4550, TVA_REDUITE)];

        // 3 x 9,99 = 29,97 HT, TVA 5,99 ; 2 x 45,50 = 91,00 HT, TVA 9,10.
        let totaux = totaliser(&lignes);
        assert_eq!(totaux.total_ht, 2997 + 9100);
        assert_eq!(totaux.total_tva, 599 + 910);
        assert_eq!(totaux.total_ttc, totaux.total_ht + totaux.total_tva);
    }

    #[test]
    fn totaliser_sans_ligne_donne_zero() {
        assert_eq!(totaliser(&[]), Totaux::default());
    }

    #[test]
    fn les_codes_de_statut_correspondent_a_la_serialisation_serde() {
        // Ces chaines partent telles quelles dans les filtres de
        // `query_documents` : un ecart avec serde ferait silencieusement
        // remonter zero resultat, sans la moindre erreur.
        for statut in [
            StatutDevis::Brouillon,
            StatutDevis::Envoye,
            StatutDevis::Accepte,
            StatutDevis::Refuse,
        ] {
            assert_eq!(serde_json::to_value(statut).unwrap(), statut.code());
        }
        for statut in [
            StatutCommande::EnPreparation,
            StatutCommande::Expediee,
            StatutCommande::Livree,
            StatutCommande::Annulee,
        ] {
            assert_eq!(serde_json::to_value(statut).unwrap(), statut.code());
        }
        for statut in [
            StatutFacture::Emise,
            StatutFacture::Payee,
            StatutFacture::EnRetard,
            StatutFacture::Annulee,
        ] {
            assert_eq!(serde_json::to_value(statut).unwrap(), statut.code());
        }
        for sens in [SensMouvement::Entree, SensMouvement::Sortie] {
            assert_eq!(serde_json::to_value(sens).unwrap(), sens.code());
        }
    }

    #[test]
    fn le_seuil_de_reapprovisionnement_se_declenche_sous_le_mini() {
        let mut article = Article {
            id: "ART-001".to_string(),
            reference: "VIS-4X30".to_string(),
            designation: "Vis 4x30".to_string(),
            famille: "quincaillerie".to_string(),
            prix_unitaire_ht: 12,
            taux_tva: TVA_NORMALE,
            stock: 10,
            stock_mini: 10,
            actif: true,
        };
        assert!(!article.sous_le_seuil(), "au seuil, rien a commander");

        article.stock = 9;
        assert!(article.sous_le_seuil());
        assert_eq!(article.valeur_stock(), 108);
    }

    #[test]
    fn le_sens_du_mouvement_donne_le_signe_applique_au_stock() {
        assert_eq!(SensMouvement::Entree.signe(), 1);
        assert_eq!(SensMouvement::Sortie.signe(), -1);
    }

    #[test]
    fn le_formatage_des_montants_reste_lisible() {
        assert_eq!(formater(123_456), "1234,56 EUR");
        assert_eq!(formater(5), "0,05 EUR");
        assert_eq!(formater(0), "0,00 EUR");
        assert_eq!(formater(-999), "-9,99 EUR");
    }
}
