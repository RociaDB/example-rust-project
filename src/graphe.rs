//! Le graphe : qui fournit quoi, quel devis porte sur quels articles, quelle
//! commande a produit quelle facture.
//!
//! Deux principes tiennent tout le module :
//!
//! - **le graphe est un index, jamais la source de verite.** Aucune RPC ne
//!   relit la valeur d'une arete (`neighbors_out` ne rend qu'un `node_id` et
//!   un `edge_id`), donc tout ce qui doit etre relu vit dans le document. Les
//!   valeurs d'aretes ecrites ici documentent la relation cote serveur ;
//!   elles ne se lisent pas.
//! - **une arete exige ses deux extremites.** `add_edge` rend `NOT_FOUND` si
//!   `from` ou `to` n'existe pas deja comme noeud : les noeuds d'abord, les
//!   aretes ensuite, toujours.

use crate::contexte::Contexte;
use crate::erreur::Resultat;
use crate::modele::{Article, Ligne};
use crate::pagination::{TAILLE_PAGE, collecter_tout};
use crate::schema::{self, NoeudArticle, RefDocument};
use rocia_db_sdk::{EdgeInput, Neighbor, NeighborNode, NodeInput};
use serde_json::json;

/// Ecrit les noeuds d'articles, enrichis, en un seul lot.
///
/// `put_nodes` envoie jusqu'a 10 requetes en parallele et prend un
/// `NodeInput` par article. `request_id: None` laisse le SDK generer une cle
/// par element ; on la fixe ici pour qu'un lot rejoue apres une coupure
/// reprenne sans dupliquer.
///
/// La valeur d'un noeud doit etre un **objet** JSON — un scalaire ou un
/// tableau est refuse avec `INVALID_ARGUMENT`. On y range le pointeur
/// `{collection, id}` qu'aurait ecrit `create_document`, plus les trois
/// champs qui rendent une traversee lisible sans relire chaque document.
pub async fn enregistrer_articles(ctx: &Contexte, articles: &[Article]) -> Resultat<()> {
    let noeuds: Vec<NodeInput> = articles
        .iter()
        .map(|article| -> Resultat<NodeInput> {
            Ok(NodeInput {
                node_id: schema::noeud(schema::LABEL_ARTICLE, &article.id),
                value: serde_json::to_value(NoeudArticle {
                    collection: schema::ARTICLES.to_string(),
                    id: article.id.clone(),
                    reference: article.reference.clone(),
                    designation: article.designation.clone(),
                    famille: article.famille.clone(),
                })?,
                request_id: Some(ctx.cle(&format!("noeud-article:{}", article.id))),
            })
        })
        .collect::<Resultat<_>>()?;

    ctx.client
        .put_nodes(&ctx.tenant, &ctx.graphe, noeuds)
        .await?;
    Ok(())
}

/// Ecrit un noeud isole. `put_node` genere sa propre cle d'idempotence.
pub async fn enregistrer_noeud(
    ctx: &Contexte,
    node_id: &str,
    valeur: &RefDocument,
) -> Resultat<()> {
    ctx.client
        .put_node(&ctx.tenant, &ctx.graphe, node_id, valeur)
        .await?;
    Ok(())
}

/// Rattache un lot d'articles a leur fournisseur.
///
/// `add_edges` est le pendant par lot de `add_edge`, avec la meme contrainte
/// d'ordre : les deux extremites doivent deja exister. La valeur de l'arete
/// porte les conditions d'achat — utile a la lecture des donnees cote
/// serveur, mais non relisible par le SDK.
pub async fn lier_fournisseur_aux_articles(
    ctx: &Contexte,
    fournisseur_id: &str,
    articles: &[(String, i64)],
) -> Resultat<()> {
    let depuis = schema::noeud(schema::LABEL_FOURNISSEUR, fournisseur_id);
    let aretes: Vec<EdgeInput> = articles
        .iter()
        .map(|(article_id, prix_achat)| {
            let vers = schema::noeud(schema::LABEL_ARTICLE, article_id);
            EdgeInput {
                edge_id: schema::arete(schema::FOURNIT, &depuis, &vers),
                from: depuis.clone(),
                to: vers,
                label: schema::FOURNIT.to_string(),
                value: json!({ "prix_achat_ht": prix_achat }),
                request_id: Some(ctx.cle(&format!("fournit:{fournisseur_id}:{article_id}"))),
            }
        })
        .collect();

    ctx.client
        .add_edges(&ctx.tenant, &ctx.graphe, aretes)
        .await?;
    Ok(())
}

/// Relie un client au devis qu'il a demande.
pub async fn lier_client_au_devis(ctx: &Contexte, client_id: &str, devis_id: &str) -> Resultat<()> {
    let depuis = schema::noeud(schema::LABEL_CLIENT, client_id);
    let vers = schema::noeud(schema::LABEL_DEVIS, devis_id);
    ctx.client
        .add_edge(
            &ctx.tenant,
            &ctx.graphe,
            &schema::arete(schema::A_DEMANDE, &depuis, &vers),
            &depuis,
            &vers,
            schema::A_DEMANDE,
            &json!({ "date": chrono::Utc::now().to_rfc3339() }),
        )
        .await?;
    Ok(())
}

/// Relie un devis aux articles qu'il chiffre.
pub async fn lier_devis_aux_lignes(
    ctx: &Contexte,
    devis_id: &str,
    lignes: &[Ligne],
) -> Resultat<()> {
    let depuis = schema::noeud(schema::LABEL_DEVIS, devis_id);
    let aretes: Vec<EdgeInput> = lignes
        .iter()
        .map(|ligne| {
            let vers = schema::noeud(schema::LABEL_ARTICLE, &ligne.article_id);
            EdgeInput {
                edge_id: schema::arete(schema::PORTE_SUR, &depuis, &vers),
                from: depuis.clone(),
                to: vers,
                label: schema::PORTE_SUR.to_string(),
                value: json!({
                    "quantite": ligne.quantite,
                    "prix_unitaire_ht": ligne.prix_unitaire_ht,
                }),
                request_id: Some(ctx.cle(&format!("porte-sur:{devis_id}:{}", ligne.article_id))),
            }
        })
        .collect();

    ctx.client
        .add_edges(&ctx.tenant, &ctx.graphe, aretes)
        .await?;
    Ok(())
}

/// Relie deux pieces commerciales : devis -> commande, commande -> facture.
///
/// `add_edge_with_request_id` fixe la cle d'idempotence : une conversion de
/// devis rejouee apres un delai reseau ne cree pas une seconde arete.
pub async fn lier_pieces(
    ctx: &Contexte,
    label_depuis: &str,
    id_depuis: &str,
    label_vers: &str,
    id_vers: &str,
    relation: &str,
) -> Resultat<()> {
    let depuis = schema::noeud(label_depuis, id_depuis);
    let vers = schema::noeud(label_vers, id_vers);
    ctx.client
        .add_edge_with_request_id(
            &ctx.tenant,
            &ctx.graphe,
            &schema::arete(relation, &depuis, &vers),
            &depuis,
            &vers,
            relation,
            &json!({ "date": chrono::Utc::now().to_rfc3339() }),
            ctx.cle(&format!("{relation}:{id_depuis}:{id_vers}")),
        )
        .await?;
    Ok(())
}

/// Les articles d'un fournisseur, valeurs de noeuds comprises.
///
/// `get_outgoing_neighbor_nodes` enchaine pour nous les deux etapes que
/// [`voisins_sortants`] laisse separees : lister les voisins, puis lire
/// chaque noeud et le deserialiser. Comme les noeuds d'articles sont
/// enrichis, la designation arrive sans un seul `get_document`.
pub async fn articles_du_fournisseur(
    ctx: &Contexte,
    fournisseur_id: &str,
) -> Resultat<Vec<NeighborNode<NoeudArticle>>> {
    Ok(ctx
        .client
        .get_outgoing_neighbor_nodes::<NoeudArticle>(
            &ctx.tenant,
            &ctx.graphe,
            &schema::noeud(schema::LABEL_FOURNISSEUR, fournisseur_id),
            schema::FOURNIT,
        )
        .await?)
}

/// Qui approvisionne un article : la meme traversee, a rebours.
pub async fn fournisseurs_de_l_article(
    ctx: &Contexte,
    article_id: &str,
) -> Resultat<Vec<NeighborNode<RefDocument>>> {
    Ok(ctx
        .client
        .get_incoming_neighbor_nodes::<RefDocument>(
            &ctx.tenant,
            &ctx.graphe,
            &schema::noeud(schema::LABEL_ARTICLE, article_id),
            schema::FOURNIT,
        )
        .await?)
}

/// Les voisins sortants bruts, page apres page.
///
/// A preferer a `get_outgoing_neighbor_nodes` des que le degre du noeud est
/// eleve : celui-ci ramene tout d'un coup, celui-la se pagine.
pub async fn voisins_sortants(
    ctx: &Contexte,
    node_id: &str,
    label: &str,
) -> Resultat<Vec<Neighbor>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    let graphe = ctx.graphe.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .neighbors_out(
                tenant,
                graphe,
                node_id,
                label,
                Some(limite),
                curseur.as_deref(),
            )
            .await?;
        Ok(page.into())
    })
    .await
}

/// Les voisins entrants bruts, page apres page.
pub async fn voisins_entrants(
    ctx: &Contexte,
    node_id: &str,
    label: &str,
) -> Resultat<Vec<Neighbor>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    let graphe = ctx.graphe.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .neighbors_in(
                tenant,
                graphe,
                node_id,
                label,
                Some(limite),
                curseur.as_deref(),
            )
            .await?;
        Ok(page.into())
    })
    .await
}

/// Le noeud tel quel, sans le contraindre a un type Rust.
///
/// `get_node` rend une `serde_json::Value` : c'est ce qu'il faut pour
/// inspecter un noeud dont on ignore la forme. `get_node_as::<T>` fait le
/// meme appel en deserialisant, et echoue en `Decode` si la forme ne
/// correspond pas.
pub async fn noeud_brut(ctx: &Contexte, node_id: &str) -> Resultat<serde_json::Value> {
    Ok(ctx
        .client
        .get_node(&ctx.tenant, &ctx.graphe, node_id)
        .await?)
}

/// Le pointeur `{collection, id}` porte par un noeud lie a un document.
pub async fn noeud_reference(ctx: &Contexte, node_id: &str) -> Resultat<RefDocument> {
    Ok(ctx
        .client
        .get_node_as::<RefDocument>(&ctx.tenant, &ctx.graphe, node_id)
        .await?)
}

/// Les graphes du tenant. Comme les collections, ils n'existent qu'a partir
/// du premier noeud ecrit.
pub async fn lister_graphes(ctx: &Contexte) -> Resultat<Vec<String>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_graphs(tenant, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

/// Tous les identifiants de noeuds du graphe.
pub async fn lister_noeuds(ctx: &Contexte) -> Resultat<Vec<String>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    let graphe = ctx.graphe.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_nodes(tenant, graphe, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

/// Supprime une arete.
///
/// `delete_edge` n'est **pas** idempotent : sur une arete absente il rend
/// `NOT_FOUND`, contrairement a `delete_document` et `delete_file`.
pub async fn delier(ctx: &Contexte, edge_id: &str) -> Resultat<()> {
    ctx.client
        .delete_edge(&ctx.tenant, &ctx.graphe, edge_id)
        .await?;
    Ok(())
}

/// Supprime une arete avec une cle d'idempotence explicite : une purge
/// relancee ne redemande pas au serveur une suppression deja acceptee.
pub async fn delier_avec_cle(ctx: &Contexte, edge_id: &str) -> Resultat<()> {
    ctx.client
        .delete_edge_with_request_id(
            &ctx.tenant,
            &ctx.graphe,
            edge_id,
            ctx.cle(&format!("suppression-arete:{edge_id}")),
        )
        .await?;
    Ok(())
}
