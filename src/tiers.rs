//! Clients et fournisseurs : le service documentaire, et la liaison
//! document -> noeud de graphe.
//!
//! Un tiers est ecrit avec `create_document`, qui fait deux choses en une :
//! il ecrit le document, puis, quand `node_label` et `node_graph` sont tous
//! deux fournis, il cree le noeud `"{label}:{id}"` portant un pointeur
//! `{collection, id}` vers ce document. C'est ce qui rend le tiers
//! immediatement traversable dans le graphe.
//!
//! Attention : les deux ecritures ne sont pas atomiques. Si la seconde
//! echoue, le document existe sans son noeud — voir [`reparer_liaisons`].

use crate::contexte::Contexte;
use crate::erreur::{ErreurApp, Resultat, est_introuvable};
use crate::modele::{Client, Fournisseur};
use crate::pagination::{TAILLE_PAGE, collecter_tout};
use crate::schema;
use rocia_db_sdk::DocumentPage;

/// Ecrit un client, et le noeud de graphe qui pointe vers lui.
pub async fn creer_client(ctx: &Contexte, client: &Client) -> Resultat<()> {
    // `create_document` prend une `Value` deja construite (et non un `&impl
    // Serialize`) : c'est la seule methode d'ecriture dans ce cas.
    ctx.client
        .create_document(
            &ctx.tenant,
            schema::CLIENTS,
            &client.id,
            serde_json::to_value(client)?,
            Some(schema::LABEL_CLIENT.to_string()),
            Some(ctx.graphe.clone()),
        )
        .await?;
    Ok(())
}

/// Ecrit un fournisseur, avec une cle d'idempotence maitrisee.
///
/// `create_document_with_request_id` est le pendant de `create_document`
/// pour un import rejouable. Deux differences a connaitre : il est generique
/// sur `Serialize` (pas besoin de passer par une `Value`), et le
/// `request_id` ne couvre *que* l'ecriture du document — la liaison au
/// noeud garde sa propre cle, generee automatiquement.
pub async fn creer_fournisseur(ctx: &Contexte, fournisseur: &Fournisseur) -> Resultat<()> {
    ctx.client
        .create_document_with_request_id(
            &ctx.tenant,
            schema::FOURNISSEURS,
            &fournisseur.id,
            fournisseur,
            Some(schema::LABEL_FOURNISSEUR.to_string()),
            Some(ctx.graphe.clone()),
            ctx.cle(&format!("fournisseur:{}", fournisseur.id)),
        )
        .await?;
    Ok(())
}

/// Relit un client. `get_document` deserialise directement dans le type
/// demande : un document qui ne colle plus au modele donne un
/// `RociaDbError::Decode`, pas un `Value` a inspecter a la main.
pub async fn lire_client(ctx: &Contexte, id: &str) -> Resultat<Client> {
    Ok(ctx
        .client
        .get_document::<Client>(&ctx.tenant, schema::CLIENTS, id)
        .await?)
}

pub async fn lire_fournisseur(ctx: &Contexte, id: &str) -> Resultat<Fournisseur> {
    Ok(ctx
        .client
        .get_document::<Fournisseur>(&ctx.tenant, schema::FOURNISSEURS, id)
        .await?)
}

/// Relit un client s'il existe, sans traiter son absence comme une erreur.
pub async fn lire_client_optionnel(ctx: &Contexte, id: &str) -> Resultat<Option<Client>> {
    match lire_client(ctx, id).await {
        Ok(client) => Ok(Some(client)),
        Err(erreur) if est_introuvable(&erreur) => Ok(None),
        Err(erreur) => Err(erreur),
    }
}

/// Tous les clients, page apres page.
pub async fn lister_clients(ctx: &Contexte) -> Resultat<Vec<Client>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_documents::<Client>(tenant, schema::CLIENTS, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

pub async fn lister_fournisseurs(ctx: &Contexte) -> Resultat<Vec<Fournisseur>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_documents::<Fournisseur>(
                tenant,
                schema::FOURNISSEURS,
                Some(limite),
                curseur.as_deref(),
            )
            .await?;
        Ok(page.into())
    })
    .await
}

/// Combien de clients, sans les ramener.
///
/// `total_count` est gratuit sur `list_documents` : le serveur tient un
/// compteur par collection, mis a jour a chaque ecriture. La meme valeur
/// coute beaucoup plus cher sur `query_documents`, qui doit filtrer
/// l'integralite des candidats pour la connaitre.
pub async fn compter_clients(ctx: &Contexte) -> Resultat<u64> {
    let page: DocumentPage<serde_json::Value> = ctx
        .client
        .list_documents(&ctx.tenant, schema::CLIENTS, Some(1), None)
        .await?;
    Ok(page.total_count)
}

/// Retrouve un client par son adresse e-mail.
///
/// `search_documents` (`FindByField`) fait une egalite exacte sur *un* champ,
/// et la valeur cherchee doit se serialiser en scalaire JSON : une chaine, un
/// nombre, un booleen ou `null`. Un objet ou un tableau part en
/// `INVALID_ARGUMENT`.
pub async fn chercher_client_par_email(ctx: &Contexte, email: &str) -> Resultat<Vec<Client>> {
    let page = ctx
        .client
        .search_documents::<Client>(
            &ctx.tenant,
            schema::CLIENTS,
            "email",
            &email,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok(page.items)
}

/// Retrouve un fournisseur par son SIRET.
pub async fn chercher_fournisseur_par_siret(
    ctx: &Contexte,
    siret: &str,
) -> Resultat<Vec<Fournisseur>> {
    let page = ctx
        .client
        .search_documents::<Fournisseur>(
            &ctx.tenant,
            schema::FOURNISSEURS,
            "siret",
            &siret,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok(page.items)
}

/// Supprime un client. `delete_document` est idempotent : supprimer un
/// identifiant absent reussit, la ou `delete_edge` rendrait `NOT_FOUND`.
pub async fn supprimer_client(ctx: &Contexte, id: &str) -> Resultat<()> {
    ctx.client
        .delete_document(&ctx.tenant, schema::CLIENTS, id)
        .await?;
    Ok(())
}

/// Supprime un fournisseur avec une cle d'idempotence explicite, pour qu'une
/// purge relancee apres une coupure ne rejoue pas les memes suppressions.
pub async fn supprimer_fournisseur(ctx: &Contexte, id: &str) -> Resultat<()> {
    ctx.client
        .delete_document_with_request_id(
            &ctx.tenant,
            schema::FOURNISSEURS,
            id,
            ctx.cle(&format!("suppression-fournisseur:{id}")),
        )
        .await?;
    Ok(())
}

/// Recree les noeuds manquants des clients.
///
/// L'ecriture document-puis-noeud de `create_document` n'est pas atomique :
/// si le second appel echoue, le document reste sans liaison. Cette fonction
/// est le rattrapage : elle relit les clients et reecrit le noeud, avec une
/// cle d'idempotence stable pour que la reparation soit elle-meme rejouable.
pub async fn reparer_liaisons(ctx: &Contexte) -> Resultat<usize> {
    let clients = lister_clients(ctx).await?;
    let mut repares = 0;
    for client in &clients {
        let node_id = schema::noeud(schema::LABEL_CLIENT, &client.id);
        let existe = match ctx
            .client
            .get_node_as::<schema::RefDocument>(&ctx.tenant, &ctx.graphe, &node_id)
            .await
        {
            Ok(_) => true,
            Err(erreur) if erreur.reason() == Some("not_found") => false,
            Err(erreur) => return Err(ErreurApp::from(erreur)),
        };
        if existe {
            continue;
        }
        // `put_node_with_request_id` : meme charge utile que celle qu'aurait
        // ecrite `create_document`, avec une cle stable liee au client.
        ctx.client
            .put_node_with_request_id(
                &ctx.tenant,
                &ctx.graphe,
                &node_id,
                &schema::RefDocument {
                    collection: schema::CLIENTS.to_string(),
                    id: client.id.clone(),
                },
                format!("reparation-noeud:{}", client.id),
            )
            .await?;
        repares += 1;
    }
    Ok(repares)
}
