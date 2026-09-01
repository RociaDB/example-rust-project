//! Purge des donnees de demonstration.
//!
//! Trois semantiques de suppression cohabitent, et c'est ce qui rend ce
//! module instructif :
//!
//! - `delete_document` et `delete_file` sont **idempotents** : supprimer ce
//!   qui n'existe pas reussit ;
//! - `delete_edge` ne l'est pas : il rend `NOT_FOUND` sur une arete absente,
//!   d'ou le filtrage explicite ici ;
//! - **rien ne supprime un noeud.** Il n'y a pas de RPC pour cela : un noeud
//!   sans arete ni document reste liste par `list_nodes`. C'est une limite
//!   du service, pas un oubli de cet exemple.

use crate::contexte::Contexte;
use crate::erreur::{Resultat, est_introuvable};
use crate::pagination::{TAILLE_PAGE, collecter_tout};
use crate::{graphe, pieces, schema};

/// Ce qui a ete supprime.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Bilan {
    pub documents: usize,
    pub aretes: usize,
    pub fichiers: usize,
    pub noeuds_orphelins: usize,
}

/// Supprime documents, aretes et fichiers de demonstration.
pub async fn purger(ctx: &Contexte) -> Resultat<Bilan> {
    // 1. Les aretes d'abord : elles se decouvrent en traversant le graphe,
    //    ce que les suppressions suivantes ne remettent pas en cause.
    let aretes = purger_aretes(ctx).await?;

    // 2. Les documents, collection par collection.
    let mut documents = 0;
    for collection in schema::TOUTES_COLLECTIONS {
        documents += purger_collection(ctx, collection).await?;
    }

    // 3. Les fichiers du bucket.
    let mut fichiers = 0;
    for file_id in pieces::lister_fichiers(ctx).await? {
        pieces::supprimer(ctx, &file_id).await?;
        fichiers += 1;
    }

    // 4. Ce qui reste : les noeuds, que le service ne sait pas supprimer.
    let noeuds_orphelins = graphe::lister_noeuds(ctx).await?.len();

    Ok(Bilan {
        documents,
        aretes,
        fichiers,
        noeuds_orphelins,
    })
}

/// Supprime tous les documents d'une collection.
async fn purger_collection(ctx: &Contexte, collection: &str) -> Resultat<usize> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    let identifiants: Vec<String> =
        collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
            let page = client
                .list_documents::<serde_json::Value>(
                    tenant,
                    collection,
                    Some(limite),
                    curseur.as_deref(),
                )
                .await?;
            Ok(page.into())
        })
        .await?
        .into_iter()
        // `list_documents` rend les documents, pas leurs identifiants : celui-ci
        // est un champ du document, par convention de ce projet.
        .filter_map(|document| {
            document
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();

    for id in &identifiants {
        ctx.client
            .delete_document(&ctx.tenant, collection, id)
            .await?;
    }
    Ok(identifiants.len())
}

/// Supprime les aretes de demonstration, en tolerant celles deja absentes.
async fn purger_aretes(ctx: &Contexte) -> Resultat<usize> {
    let noeuds = graphe::lister_noeuds(ctx).await?;
    let labels = [
        schema::FOURNIT,
        schema::A_DEMANDE,
        schema::PORTE_SUR,
        schema::CONVERTI_EN,
        schema::FACTURE_PAR,
    ];

    let mut supprimees = 0;
    for depuis in &noeuds {
        for label in labels {
            for voisin in graphe::voisins_sortants(ctx, depuis, label).await? {
                // `Neighbor` porte l'identifiant reel de l'arete : inutile de
                // le recomposer avec `schema::arete`, qui ne sert que du cote
                // ecriture, quand l'arete n'existe pas encore.
                match graphe::delier_avec_cle(ctx, &voisin.edge_id).await {
                    Ok(()) => supprimees += 1,
                    // `delete_edge` n'est pas idempotent : une arete deja
                    // partie (purge relancee, suppression concurrente) rend
                    // `NOT_FOUND`, ce qui n'est pas un echec ici.
                    Err(erreur) if est_introuvable(&erreur) => {}
                    Err(erreur) => return Err(erreur),
                }
            }
        }
    }
    Ok(supprimees)
}
