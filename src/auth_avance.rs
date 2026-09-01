//! Le module `auth` du SDK, utilise sans `RociaDbClient`.
//!
//! `RociaDbBuilder::build()` monte tout cela pour vous. Le module est public
//! pour le cas ou le meme jeton doit servir ailleurs — un service HTTP
//! interne a cote de RociaDB, typiquement : on obtient un jeton, on partage
//! le meme gestionnaire, on le laisse se renouveler tout seul.

use crate::erreur::Resultat;
use rocia_db_sdk::auth::{ApiKeyInterceptor, TokenManager, fetch_token};
use std::time::Duration;

/// Ce que la demonstration a observe du cycle de vie du jeton.
pub struct Observation {
    pub type_de_jeton: String,
    pub duree_de_vie: Duration,
    pub intervalle_de_rafraichissement: Duration,
    pub longueur_du_jeton: usize,
}

/// Obtient un jeton, monte un gestionnaire, et lance son rafraichissement.
pub async fn demontrer(
    token_url: &str,
    client_id: &str,
    client_secret: &str,
) -> Resultat<Observation> {
    let http = reqwest::Client::new();

    // 1. Un jeton, une fois. `fetch_token` est l'appel brut : rien n'est mis
    //    en cache, rien n'est renouvele.
    let jeton = fetch_token(&http, token_url, client_id, client_secret).await?;

    // 2. Le gestionnaire : il obtient son propre premier jeton des la
    //    construction, et garde l'en-tete `Authorization` pret a l'emploi.
    let gestionnaire = TokenManager::new(
        http,
        token_url.to_string(),
        client_id.to_string(),
        client_secret.to_string(),
    )
    .await?;

    // 3. L'intervalle sur : `max(expires_in * 2/3, 5 s)`, soit un
    //    renouvellement alors qu'il reste encore un tiers de la duree de vie.
    let intervalle = gestionnaire.refresh_interval();

    // 4. La tache de fond. Le garde rendu est `#[must_use]` a bon escient :
    //    le laisser tomber arrete le rafraichissement sur-le-champ.
    let _garde = gestionnaire.spawn_refresh(intervalle);

    // 5. L'intercepteur qui injecte l'en-tete dans chaque appel gRPC — c'est
    //    exactement celui que le builder installe sur les quatre services.
    let _intercepteur = gestionnaire.interceptor();

    // 6. Un renouvellement immediat, et un signal « ce jeton est suspect »
    //    que la tache de fond absorbe sans bloquer l'appelant.
    gestionnaire.refresh_now().await?;
    gestionnaire.request_refresh();

    // 7. Cote serveur, `ApiKeyInterceptor` valide un en-tete `x-api-key`
    //    entrant. Il n'a rien a faire dans un client — il est ici pour
    //    montrer que le module `auth` couvre les deux bouts.
    let _cote_serveur = ApiKeyInterceptor::new("cle-de-service".to_string());

    Ok(Observation {
        type_de_jeton: jeton.token_type,
        duree_de_vie: Duration::from_secs(jeton.expires_in),
        intervalle_de_rafraichissement: intervalle,
        longueur_du_jeton: jeton.access_token.len(),
    })
    // `_garde` tombe ici : la tache de fond s'arrete avec lui. Dans une vraie
    // application, il vit aussi longtemps que le gestionnaire sert.
}
