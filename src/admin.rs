//! Exploration du deploiement, et cycle de vie du jeton.

use crate::contexte::Contexte;
use crate::erreur::Resultat;
use crate::pagination::{TAILLE_PAGE, collecter_tout};

/// Les tenants connus du deploiement.
///
/// Seule RPC non rattachee a un tenant : elle enumere tout le deploiement, et
/// une politique d'autorisation dediee peut la refuser. D'ou le `Option` :
/// un `PERMISSION_DENIED` ici veut dire « pas votre role », pas « en panne ».
///
/// A retenir : `tenant_id` est une partition metier, **pas** une frontiere de
/// securite. Il n'est deduit d'aucune identite — n'importe quel client
/// authentifie peut adresser n'importe quel tenant. C'est a l'application
/// d'imposer qui a le droit de toucher a quoi.
pub async fn tenants(ctx: &Contexte) -> Resultat<Option<Vec<String>>> {
    match ctx.client.list_tenants(Some(TAILLE_PAGE), None).await {
        Ok(page) => Ok(Some(page.items)),
        Err(erreur) if erreur.is_permission_denied() => Ok(None),
        Err(erreur) => Err(erreur.into()),
    }
}

/// Les buckets du tenant, tous confondus.
pub async fn buckets(ctx: &Contexte) -> Resultat<Vec<String>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_buckets(tenant, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

/// Force un renouvellement du jeton, tout de suite.
///
/// Le SDK rafraichit deja en tache de fond (`max(expires_in * 2/3, 5 s)`).
/// Cet appel sert au cas ou un appel vient d'echouer en `UNAUTHENTICATED` :
/// on renouvelle, puis on rejoue. Sans authentification, il ne fait rien.
pub async fn rafraichir_jeton(ctx: &Contexte) -> Resultat<()> {
    ctx.client.refresh_auth_token().await?;
    Ok(())
}

/// Marque le jeton en cache comme suspect, sans payer l'aller-retour.
///
/// La tache de fond absorbe le renouvellement ; c'est le pendant « je ne
/// fais plus confiance a ce jeton » de [`rafraichir_jeton`], qui, lui,
/// attend le nouveau jeton.
pub fn invalider_jeton(ctx: &Contexte) {
    ctx.client.invalidate_auth_token();
}

/// Rejoue un appel apres renouvellement du jeton, une seule fois.
///
/// C'est la seule reprise qui ait un sens sur une erreur d'authentification :
/// `UNAUTHENTICATED` est temporaire (jeton expire), `PERMISSION_DENIED` est
/// definitif (portee insuffisante) — rejouer ne changerait rien au second.
pub async fn avec_reprise<T, F, Fut>(ctx: &Contexte, mut appel: F) -> Resultat<T>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Resultat<T>>,
{
    match appel().await {
        Err(crate::erreur::ErreurApp::Rocia(erreur)) if erreur.is_unauthenticated() => {
            ctx.client.refresh_auth_token().await?;
            appel().await
        }
        autre => autre,
    }
}
