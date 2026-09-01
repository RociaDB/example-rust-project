//! Erreur applicative, et lecture d'une `RociaDbError`.
//!
//! `RociaDbError` est une enumeration typee, pas un `Box<dyn Error>` : on
//! peut donc decider quoi faire en filtrant dessus, sans downcast. Ce module
//! montre les trois questions qu'on lui pose en pratique :
//!
//! 1. est-ce reessayable apres rafraichissement du jeton
//!    (`is_unauthenticated`) ?
//! 2. est-ce definitif faute de portee (`is_permission_denied`) ?
//! 3. sinon, qu'a exactement repondu le serveur (`code`, `reason`) ?

use rocia_db_sdk::RociaDbError;

pub type Resultat<T> = std::result::Result<T, ErreurApp>;

#[derive(Debug, thiserror::Error)]
pub enum ErreurApp {
    /// Toute erreur remontee par le SDK.
    #[error(transparent)]
    Rocia(#[from] RociaDbError),

    /// Serialisation d'un objet metier avant envoi.
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),

    /// Flux gRPC brut (`download_file_stream` rend un `tonic::Status`, pas
    /// une `RociaDbError` : c'est le seul endroit ou il faut le traduire).
    #[error("flux de telechargement interrompu: {0}")]
    Flux(String),

    /// Regle metier violee avant tout appel reseau.
    #[error("{0}")]
    Metier(String),
}

impl ErreurApp {
    pub fn metier(message: impl Into<String>) -> Self {
        Self::Metier(message.into())
    }
}

/// Vrai quand le serveur a repondu `NOT_FOUND`.
///
/// `reason()` rend la metadonnee de fin envoyee par le serveur (`not_found`,
/// `invalid_argument`, `already_exists`, ...), plus fine que le code gRPC
/// seul, et surtout comparable sans dependre du crate `tonic`.
pub fn est_introuvable(erreur: &ErreurApp) -> bool {
    match erreur {
        ErreurApp::Rocia(rocia) => rocia.reason() == Some("not_found"),
        _ => false,
    }
}

/// Explication lisible d'une erreur, avec la conduite a tenir.
///
/// C'est la fonction qui filtre sur *toutes* les variantes de
/// `RociaDbError` : elle sert de table de correspondance entre ce que rend
/// le SDK et ce qu'un operateur doit faire.
pub fn expliquer(erreur: &ErreurApp) -> String {
    let rocia = match erreur {
        ErreurApp::Rocia(rocia) => rocia,
        ErreurApp::Json(source) => {
            return format!("Objet metier non serialisable ({source}) : corrigez le modele.");
        }
        ErreurApp::Flux(message) => {
            return format!(
                "Le flux gRPC s'est interrompu ({message}) : relancez le telechargement."
            );
        }
        ErreurApp::Metier(message) => return format!("Regle metier : {message}"),
    };

    // Les deux questions a poser en premier : elles decident d'un reessai.
    if rocia.is_unauthenticated() {
        return format!(
            "Jeton invalide ou expire ({rocia}). Appelez `refresh_auth_token()` \
             puis rejouez l'appel : c'est le seul cas ou reessayer a un sens."
        );
    }
    if rocia.is_permission_denied() {
        return format!(
            "Portee insuffisante ({rocia}). Rafraichir le jeton n'y changera rien : \
             un jeton en lecture seule est refuse sur les 7 RPC d'ecriture, et un \
             jeton d'administration l'est sur les 22. Verifiez le `client_id` utilise."
        );
    }

    match rocia {
        RociaDbError::Status { operation, .. } => {
            // Trois accesseurs, du plus grossier au plus precis :
            // `code()` rend le code gRPC, `reason()` la raison detaillee
            // envoyee par le serveur en metadonnee de fin, et `status()` le
            // `tonic::Status` complet — rien n'est perdu par rapport a un
            // appel direct du client genere.
            let code = rocia
                .code()
                .map_or_else(|| "?".to_string(), |code| code.description().to_string());
            let raison = rocia.reason().unwrap_or("(sans raison)");
            let detail = rocia.status().map_or("", |status| status.message());
            format!(
                "Le serveur a refuse l'operation « {operation} » : {code} / {raison} — {detail}"
            )
        }
        RociaDbError::Connection { message, .. } => format!(
            "Connexion impossible : {message}. Verifiez que le serveur ecoute, et que \
             l'hote ne porte ni chemin ni suffixe (« http://127.0.0.1:50051 », pas « /v1 »)."
        ),
        RociaDbError::Auth { message, .. } => format!(
            "Obtention du jeton impossible : {message}. Verifiez AUTH_TOKEN_URL, \
             AUTH_CLIENT_ID et AUTH_CLIENT_SECRET, ou passez --sans-auth en local."
        ),
        RociaDbError::Encode { context, source } => {
            format!("Encodage JSON de « {context} » impossible : {source}")
        }
        RociaDbError::Decode { context, source } => format!(
            "Decodage JSON de « {context} » impossible : {source}. Le document stocke \
             ne correspond plus au type Rust demande."
        ),
        RociaDbError::Validation(message) => {
            format!("Rejete par le SDK avant tout appel reseau : {message}. Rien n'a ete envoye.")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_erreur_de_validation_est_expliquee_sans_serveur() {
        // `page_request` refuse une limite nulle cote client : le SDK rend
        // une `Validation`, jamais un aller-retour reseau.
        let erreur = ErreurApp::Rocia(RociaDbError::Validation(
            "page limit must be greater than zero".to_string(),
        ));
        let texte = expliquer(&erreur);
        assert!(texte.contains("avant tout appel reseau"), "{texte}");
        assert!(!est_introuvable(&erreur));
    }

    #[test]
    fn une_erreur_metier_est_rendue_telle_quelle() {
        let erreur = ErreurApp::metier("stock insuffisant sur ART-001");
        assert_eq!(
            expliquer(&erreur),
            "Regle metier : stock insuffisant sur ART-001"
        );
    }

    #[test]
    fn une_erreur_json_n_est_pas_prise_pour_une_erreur_serveur() {
        let source = serde_json::from_str::<crate::modele::Article>("{}").unwrap_err();
        let erreur = ErreurApp::Json(source);
        assert!(expliquer(&erreur).contains("non serialisable"));
        assert!(!est_introuvable(&erreur));
    }
}
