//! Construction du client, et contexte partage par tous les modules.

use crate::erreur::Resultat;
use crate::schema;
use rocia_db_sdk::{RociaDbBuilder, RociaDbClient};
use std::time::Duration;

/// Comment le client s'authentifie aupres du serveur.
#[derive(Debug, Clone)]
pub enum ModeAuth {
    /// `disable_auth()` : aucun jeton n'est demande ni envoye. Reserve au
    /// developpement local ou a un segment reseau deja chiffre.
    Desactivee,
    /// Comportement par defaut du builder : il lit lui-meme `AUTH_TOKEN_URL`,
    /// `AUTH_CLIENT_ID` et `AUTH_CLIENT_SECRET` au moment du `build()`.
    VariablesEnv,
    /// `auth_client_credentials(..)` : les trois valeurs sont fournies
    /// explicitement, par exemple parce qu'elles viennent d'un coffre.
    Explicite {
        token_url: String,
        client_id: String,
        client_secret: String,
    },
}

/// Tout ce dont les modules metier ont besoin : le client, le tenant, et les
/// noms d'espaces ou ecrire.
pub struct Contexte {
    pub client: RociaDbClient,
    pub tenant: String,
    pub graphe: String,
    pub bucket: String,
    /// Prefixe des cles d'idempotence de cette execution. Voir [`Contexte::cle`].
    pub job: String,
}

// `RociaDbClient` ne derive pas `Debug` (il porte un canal gRPC et un
// gestionnaire de jeton), donc l'impl est ecrite a la main sur ce qui est
// utile a lire — et rien qui ressemble a un secret.
impl std::fmt::Debug for Contexte {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Contexte")
            .field("tenant", &self.tenant)
            .field("graphe", &self.graphe)
            .field("bucket", &self.bucket)
            .field("job", &self.job)
            .finish_non_exhaustive()
    }
}

impl Contexte {
    /// Ouvre la connexion.
    ///
    /// `build()` est faillible pour deux raisons distinctes : l'hote est
    /// invalide ou injoignable (`Connection`), ou le jeton n'a pas pu etre
    /// obtenu (`Auth`). Les deux remontent avant le moindre appel metier.
    pub async fn ouvrir(
        hote: &str,
        tenant: &str,
        auth: ModeAuth,
        delai_connexion: Duration,
    ) -> Resultat<Self> {
        // Le builder mute en place et rend `&mut Self` : il se garde dans une
        // variable, il ne se chaine pas jusqu'au `build()`.
        let mut builder = RociaDbBuilder::new();
        builder.host(hote);
        builder.connect_timeout(delai_connexion);

        match &auth {
            ModeAuth::Desactivee => {
                builder.disable_auth();
            }
            ModeAuth::VariablesEnv => {}
            ModeAuth::Explicite {
                token_url,
                client_id,
                client_secret,
            } => {
                builder.auth_client_credentials(token_url, client_id, client_secret);
            }
        }

        // `RociaDbClient` se clone a bas cout : les clones partagent le canal,
        // le gestionnaire de jeton et la tache de rafraichissement. Chaque
        // methode prend `&self`, donc un client derriere un `Arc` se partage
        // entre taches sans `Mutex`.
        let client = builder.build().await?;

        Ok(Self {
            client,
            tenant: tenant.to_string(),
            graphe: "erp".to_string(),
            bucket: schema::BUCKET_PIECES.to_string(),
            job: job_par_defaut(),
        })
    }

    /// Cle d'idempotence, prefixee par l'identifiant de cette execution.
    ///
    /// Le serveur deduplique sur `(tenant, operation, request_id)` pendant
    /// `gc.request_ttl_secs` (24 h par defaut). Une cle *stable* est donc ce
    /// qu'il faut pour rejouer sans risque un import interrompu — mais si la
    /// demonstration reutilisait les memes cles a chaque lancement, un second
    /// `demo` apres un `nettoyer` ne reecrirait rien du tout : le serveur y
    /// verrait le rejeu des ecritures de la veille.
    ///
    /// D'ou le compromis : cle stable *a l'interieur* d'une execution, unique
    /// d'une execution a l'autre. `--job` fige le prefixe pour retrouver le
    /// vrai comportement d'un import rejouable.
    pub fn cle(&self, suffixe: &str) -> String {
        format!("{}:{suffixe}", self.job)
    }

    /// Fige le prefixe des cles d'idempotence.
    pub fn avec_job(mut self, job: Option<String>) -> Self {
        if let Some(job) = job {
            self.job = job;
        }
        self
    }
}

fn job_par_defaut() -> String {
    format!("demo-{}", chrono::Utc::now().format("%Y%m%dT%H%M%S"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_prefixe_par_defaut_change_d_une_execution_a_l_autre() {
        let job = job_par_defaut();
        assert!(job.starts_with("demo-"), "{job}");
        assert_eq!(job.len(), "demo-".len() + "20260901T120000".len());
    }

    #[tokio::test]
    async fn un_hote_avec_chemin_est_refuse_avant_toute_connexion() {
        // Le SDK valide le chemin de l'hote cote client : tonic ignorerait
        // silencieusement le « /v1 », le serveur ne serait jamais contacte.
        let erreur = Contexte::ouvrir(
            "http://127.0.0.1:50051/v1",
            "demo-erp",
            ModeAuth::Desactivee,
            Duration::from_secs(1),
        )
        .await
        .expect_err("un hote porteur d'un chemin doit etre refuse");

        assert!(
            crate::erreur::expliquer(&erreur).contains("Connexion impossible"),
            "{erreur}"
        );
    }

    #[tokio::test]
    async fn un_delai_de_connexion_nul_est_refuse() {
        let erreur = Contexte::ouvrir(
            "http://127.0.0.1:50051",
            "demo-erp",
            ModeAuth::Desactivee,
            Duration::ZERO,
        )
        .await
        .expect_err("un delai nul doit etre refuse");

        assert!(
            crate::erreur::expliquer(&erreur).contains("avant tout appel reseau"),
            "{erreur}"
        );
    }
}
