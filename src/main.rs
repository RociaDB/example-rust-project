//! Point d'entree en ligne de commande.

use clap::{Parser, Subcommand};
use erp_rocia_db::contexte::{Contexte, ModeAuth};
use erp_rocia_db::erreur::{ErreurApp, Resultat};
use erp_rocia_db::{auth_avance, nettoyage, scenario};
use std::process::ExitCode;
use std::time::Duration;

#[derive(Parser)]
#[command(
    name = "erp-rocia-db",
    about = "ERP d'exemple (devis, commandes, factures, stock) sur RociaDB",
    long_about = "Chaque sous-commande met en avant une famille de fonctionnalites du SDK. \
                  « demo » les enchaine toutes ; les autres supposent que « demo » ou \
                  « seed » a deja tourne."
)]
struct Cli {
    /// Hote du serveur RociaDB : un nom et un port, sans chemin.
    #[arg(long, env = "ROCIA_HOST", default_value = "http://127.0.0.1:50051")]
    hote: String,

    /// Tenant vise. C'est une partition metier, pas une frontiere de securite.
    #[arg(long, env = "ROCIA_TENANT", default_value = "demo-erp")]
    tenant: String,

    /// Ne demande aucun jeton (developpement local).
    #[arg(long)]
    sans_auth: bool,

    /// URL du point de jeton OAuth2. A defaut, le SDK lit AUTH_TOKEN_URL.
    #[arg(long, env = "AUTH_TOKEN_URL")]
    token_url: Option<String>,

    /// Identifiant client OAuth2. A defaut, le SDK lit AUTH_CLIENT_ID.
    #[arg(long, env = "AUTH_CLIENT_ID")]
    client_id: Option<String>,

    /// Secret client OAuth2. A defaut, le SDK lit AUTH_CLIENT_SECRET.
    #[arg(long, env = "AUTH_CLIENT_SECRET", hide_env_values = true)]
    client_secret: Option<String>,

    /// Delai d'etablissement de la connexion, en secondes.
    #[arg(long, default_value_t = 10)]
    delai_connexion: u64,

    /// Fige le prefixe des cles d'idempotence, pour rejouer un import a
    /// l'identique. Par defaut, un prefixe different a chaque execution.
    #[arg(long)]
    job: Option<String>,

    #[command(subcommand)]
    commande: Commande,
}

#[derive(Subcommand)]
enum Commande {
    /// Le scenario complet, de l'appel d'offres au reglement.
    Demo {
        /// Purge les donnees a la fin.
        #[arg(long)]
        nettoyer: bool,
    },
    /// Tiers, catalogue et approvisionnements.
    Seed,
    /// Les quatre facons de lire des documents.
    Catalogue,
    /// Devis, commande, expedition, facture, reglement.
    Ventes,
    /// Traversees de graphe.
    Graphe,
    /// Pieces jointes : televersement et telechargement.
    Pieces,
    /// Tenants, collections, graphes, buckets, jeton.
    Admin,
    /// Le module `auth` du SDK sans `RociaDbClient` (necessite des identifiants).
    Auth,
    /// Supprime les donnees de demonstration.
    Nettoyer,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Le SDK trace ses appels : `RUST_LOG=erp_rocia_db=info,rocia_db_sdk=debug`
    // montre chaque RPC, sa cible et son issue.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn")),
        )
        .init();

    let cli = Cli::parse();
    match executer(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(erreur) => {
            eprintln!(
                "\n\x1b[31mEchec\x1b[0m : {}",
                erp_rocia_db::erreur::expliquer(&erreur)
            );
            ExitCode::FAILURE
        }
    }
}

async fn executer(cli: Cli) -> Resultat<()> {
    // La demonstration du module `auth` n'ouvre aucune connexion gRPC.
    if let Commande::Auth = cli.commande {
        return demontrer_auth(&cli).await;
    }

    let ctx = Contexte::ouvrir(
        &cli.hote,
        &cli.tenant,
        mode_auth(&cli),
        Duration::from_secs(cli.delai_connexion),
    )
    .await?
    .avec_job(cli.job.clone());

    match cli.commande {
        Commande::Demo { nettoyer } => scenario::jouer(&ctx, nettoyer).await,
        Commande::Seed => scenario::semer(&ctx).await,
        Commande::Catalogue => scenario::interroger_le_catalogue(&ctx).await,
        Commande::Ventes => scenario::vendre(&ctx).await,
        Commande::Graphe => scenario::traverser_le_graphe(&ctx).await,
        Commande::Pieces => scenario::joindre_les_pieces(&ctx).await,
        Commande::Admin => scenario::explorer(&ctx).await,
        Commande::Nettoyer => {
            let bilan = nettoyage::purger(&ctx).await?;
            println!(
                "{} document(s), {} arete(s), {} fichier(s) supprimes ; \
                 {} noeud(s) subsistent (aucune RPC ne supprime un noeud).",
                bilan.documents, bilan.aretes, bilan.fichiers, bilan.noeuds_orphelins
            );
            Ok(())
        }
        Commande::Auth => unreachable!("traite avant l'ouverture de la connexion"),
    }
}

/// Choisit le mode d'authentification a partir des options.
///
/// Trois cas, dans cet ordre : `--sans-auth` coupe tout ; des identifiants
/// complets sont passes explicitement ; sinon on laisse le builder lire
/// lui-meme `AUTH_TOKEN_URL`, `AUTH_CLIENT_ID` et `AUTH_CLIENT_SECRET`.
fn mode_auth(cli: &Cli) -> ModeAuth {
    if cli.sans_auth {
        return ModeAuth::Desactivee;
    }
    match (&cli.token_url, &cli.client_id, &cli.client_secret) {
        (Some(token_url), Some(client_id), Some(client_secret)) => ModeAuth::Explicite {
            token_url: token_url.clone(),
            client_id: client_id.clone(),
            client_secret: client_secret.clone(),
        },
        _ => ModeAuth::VariablesEnv,
    }
}

async fn demontrer_auth(cli: &Cli) -> Resultat<()> {
    let (Some(token_url), Some(client_id), Some(client_secret)) =
        (&cli.token_url, &cli.client_id, &cli.client_secret)
    else {
        return Err(ErreurApp::metier(
            "cette commande a besoin de --token-url, --client-id et --client-secret \
             (ou de AUTH_TOKEN_URL, AUTH_CLIENT_ID et AUTH_CLIENT_SECRET)",
        ));
    };

    let observation = auth_avance::demontrer(token_url, client_id, client_secret).await?;
    println!(
        "Jeton obtenu : type « {} », {} caracteres",
        observation.type_de_jeton, observation.longueur_du_jeton
    );
    println!("Duree de vie annoncee : {:?}", observation.duree_de_vie);
    println!(
        "Intervalle de rafraichissement retenu par le SDK : {:?} (soit expires_in x 2/3)",
        observation.intervalle_de_rafraichissement
    );
    Ok(())
}
