//! Le flux commercial : devis -> bon de commande -> facture.
//!
//! Chaque piece est un document, et chaque transition ecrit une arete. Le
//! document reste la source de verite (lignes, totaux, statut) ; le graphe
//! sert a remonter la chaine : de la facture a la commande, de la commande au
//! devis, du devis au client.

use crate::catalogue;
use crate::contexte::Contexte;
use crate::erreur::{ErreurApp, Resultat};
use crate::graphe;
use crate::modele::{
    BonCommande, Devis, Facture, Ligne, SensMouvement, StatutCommande, StatutDevis, StatutFacture,
    totaliser,
};
use crate::pagination::TAILLE_PAGE;
use crate::schema;
use rocia_db_sdk::{
    DocumentQueryFilter, DocumentQueryOperator, DocumentQuerySort, DocumentQuerySortDirection,
};
use serde_json::json;

/// Etablit un devis : le document, son noeud, et les aretes vers le client et
/// les articles chiffres.
pub async fn etablir_devis(
    ctx: &Contexte,
    devis_id: &str,
    client_id: &str,
    lignes: Vec<Ligne>,
) -> Resultat<Devis> {
    if lignes.is_empty() {
        return Err(ErreurApp::metier(format!(
            "le devis {devis_id} n'a aucune ligne"
        )));
    }

    let aujourd_hui = chrono::Utc::now().date_naive();
    let devis = Devis {
        id: devis_id.to_string(),
        client_id: client_id.to_string(),
        statut: StatutDevis::Envoye,
        date: aujourd_hui.to_string(),
        validite: (aujourd_hui + chrono::Duration::days(30)).to_string(),
        totaux: totaliser(&lignes),
        lignes,
    };

    ctx.client
        .create_document_with_request_id(
            &ctx.tenant,
            schema::DEVIS,
            &devis.id,
            &devis,
            Some(schema::LABEL_DEVIS.to_string()),
            Some(ctx.graphe.clone()),
            ctx.cle(&format!("devis:{devis_id}")),
        )
        .await?;

    // Les aretes viennent apres : leurs deux extremites doivent exister.
    graphe::lier_client_au_devis(ctx, client_id, devis_id).await?;
    graphe::lier_devis_aux_lignes(ctx, devis_id, &devis.lignes).await?;

    Ok(devis)
}

/// Accepte un devis et le convertit en bon de commande.
pub async fn accepter_devis(
    ctx: &Contexte,
    devis_id: &str,
    commande_id: &str,
) -> Resultat<BonCommande> {
    let mut devis = lire_devis(ctx, devis_id).await?;
    if devis.statut == StatutDevis::Accepte {
        return Err(ErreurApp::metier(format!(
            "le devis {devis_id} est deja accepte"
        )));
    }

    devis.statut = StatutDevis::Accepte;
    ctx.client
        .put_document(&ctx.tenant, schema::DEVIS, &devis.id, &devis)
        .await?;

    let commande = BonCommande {
        id: commande_id.to_string(),
        client_id: devis.client_id.clone(),
        devis_id: Some(devis.id.clone()),
        statut: StatutCommande::EnPreparation,
        date: chrono::Utc::now().date_naive().to_string(),
        totaux: devis.totaux,
        lignes: devis.lignes.clone(),
    };
    ctx.client
        .create_document_with_request_id(
            &ctx.tenant,
            schema::BONS_COMMANDE,
            &commande.id,
            &commande,
            Some(schema::LABEL_BON_COMMANDE.to_string()),
            Some(ctx.graphe.clone()),
            ctx.cle(&format!("commande:{commande_id}")),
        )
        .await?;

    graphe::lier_pieces(
        ctx,
        schema::LABEL_DEVIS,
        devis_id,
        schema::LABEL_BON_COMMANDE,
        commande_id,
        schema::CONVERTI_EN,
    )
    .await?;

    Ok(commande)
}

/// Expedie une commande : un mouvement de sortie par ligne, puis le statut.
///
/// Le stock est decremente avant le changement de statut : si une ligne
/// manque, la commande reste « en preparation » et rien n'a bouge au-dela des
/// lignes deja traitees, qui restent tracees dans les mouvements.
pub async fn expedier(ctx: &Contexte, commande_id: &str) -> Resultat<BonCommande> {
    let mut commande = lire_commande(ctx, commande_id).await?;
    if commande.statut != StatutCommande::EnPreparation {
        return Err(ErreurApp::metier(format!(
            "la commande {commande_id} n'est pas en preparation (statut : {})",
            commande.statut.code()
        )));
    }

    for ligne in &commande.lignes {
        catalogue::mouvementer(
            ctx,
            &ligne.article_id,
            SensMouvement::Sortie,
            ligne.quantite,
            commande_id,
        )
        .await?;
    }

    commande.statut = StatutCommande::Expediee;
    ctx.client
        .put_document(&ctx.tenant, schema::BONS_COMMANDE, &commande.id, &commande)
        .await?;
    Ok(commande)
}

/// Facture une commande expediee.
pub async fn facturer(ctx: &Contexte, commande_id: &str, facture_id: &str) -> Resultat<Facture> {
    let commande = lire_commande(ctx, commande_id).await?;
    if commande.statut == StatutCommande::Annulee {
        return Err(ErreurApp::metier(format!(
            "la commande {commande_id} est annulee, elle ne peut pas etre facturee"
        )));
    }

    let aujourd_hui = chrono::Utc::now().date_naive();
    let facture = Facture {
        id: facture_id.to_string(),
        client_id: commande.client_id.clone(),
        bon_commande_id: commande.id.clone(),
        statut: StatutFacture::Emise,
        date: aujourd_hui.to_string(),
        date_echeance: (aujourd_hui + chrono::Duration::days(30)).to_string(),
        totaux: commande.totaux,
        lignes: commande.lignes.clone(),
    };

    ctx.client
        .create_document_with_request_id(
            &ctx.tenant,
            schema::FACTURES,
            &facture.id,
            &facture,
            Some(schema::LABEL_FACTURE.to_string()),
            Some(ctx.graphe.clone()),
            ctx.cle(&format!("facture:{facture_id}")),
        )
        .await?;

    graphe::lier_pieces(
        ctx,
        schema::LABEL_BON_COMMANDE,
        commande_id,
        schema::LABEL_FACTURE,
        facture_id,
        schema::FACTURE_PAR,
    )
    .await?;

    Ok(facture)
}

/// Solde une facture. La cle d'idempotence est stable et porte le numero de
/// facture : un encaissement rejoue ne repasse pas deux fois le meme
/// reglement.
pub async fn encaisser(ctx: &Contexte, facture_id: &str) -> Resultat<Facture> {
    let mut facture = lire_facture(ctx, facture_id).await?;
    facture.statut = StatutFacture::Payee;
    ctx.client
        .put_document_with_request_id(
            &ctx.tenant,
            schema::FACTURES,
            &facture.id,
            &facture,
            ctx.cle(&format!("encaissement:{facture_id}")),
        )
        .await?;
    Ok(facture)
}

pub async fn lire_devis(ctx: &Contexte, id: &str) -> Resultat<Devis> {
    Ok(ctx
        .client
        .get_document::<Devis>(&ctx.tenant, schema::DEVIS, id)
        .await?)
}

pub async fn lire_commande(ctx: &Contexte, id: &str) -> Resultat<BonCommande> {
    Ok(ctx
        .client
        .get_document::<BonCommande>(&ctx.tenant, schema::BONS_COMMANDE, id)
        .await?)
}

pub async fn lire_facture(ctx: &Contexte, id: &str) -> Resultat<Facture> {
    Ok(ctx
        .client
        .get_document::<Facture>(&ctx.tenant, schema::FACTURES, id)
        .await?)
}

/// Les factures a relancer, de la plus ancienne echeance a la plus recente.
///
/// `In` accepte plusieurs valeurs sur un meme champ, la ou `Eq` en attend
/// une. Le tri est fait par le serveur, sur un champ date au format ISO :
/// c'est ce format qui rend le tri lexicographique equivalent au tri
/// chronologique.
pub async fn factures_a_relancer(ctx: &Contexte) -> Resultat<(Vec<Facture>, u64)> {
    let filtres = [DocumentQueryFilter {
        field: "statut".to_string(),
        operator: DocumentQueryOperator::In,
        values: vec![
            json!(StatutFacture::Emise.code()),
            json!(StatutFacture::EnRetard.code()),
        ],
    }];
    let tri = [DocumentQuerySort {
        field: "date_echeance".to_string(),
        direction: DocumentQuerySortDirection::Asc,
    }];

    let page = ctx
        .client
        .query_documents::<Facture>(
            &ctx.tenant,
            schema::FACTURES,
            &filtres,
            &tri,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok((page.items, page.total_count))
}

/// Les pieces d'un client : ses devis, via le graphe.
pub async fn devis_du_client(ctx: &Contexte, client_id: &str) -> Resultat<Vec<String>> {
    let voisins = graphe::voisins_sortants(
        ctx,
        &schema::noeud(schema::LABEL_CLIENT, client_id),
        schema::A_DEMANDE,
    )
    .await?;
    Ok(voisins.into_iter().map(|voisin| voisin.node_id).collect())
}

/// Remonte de la facture a la commande qui l'a produite.
pub async fn commande_de_la_facture(ctx: &Contexte, facture_id: &str) -> Resultat<Option<String>> {
    let voisins = graphe::voisins_entrants(
        ctx,
        &schema::noeud(schema::LABEL_FACTURE, facture_id),
        schema::FACTURE_PAR,
    )
    .await?;
    Ok(voisins.into_iter().next().map(|voisin| voisin.node_id))
}

/// Annule un devis : le document part, et l'arete vers le client aussi.
///
/// L'ordre est choisi : `delete_document` est idempotent, `delete_edge` ne
/// l'est pas. En commencant par le document, une reprise apres coupure
/// repasse sans bruit sur la suppression deja faite puis termine l'arete.
/// Dans l'autre sens, la reprise buterait sur un `NOT_FOUND` — c'est aussi
/// pourquoi rejouer cette fonction sur un devis deja annule echoue, la ou la
/// purge de [`crate::nettoyage`] tolere explicitement l'arete absente.
pub async fn annuler_devis(ctx: &Contexte, devis_id: &str, client_id: &str) -> Resultat<()> {
    ctx.client
        .delete_document_with_request_id(
            &ctx.tenant,
            schema::DEVIS,
            devis_id,
            ctx.cle(&format!("annulation-devis:{devis_id}")),
        )
        .await?;

    let noeud_devis = schema::noeud(schema::LABEL_DEVIS, devis_id);

    // Les aretes vers les articles se decouvrent par traversee : `Neighbor`
    // porte l'identifiant reel de l'arete, il n'y a rien a recomposer.
    for voisin in graphe::voisins_sortants(ctx, &noeud_devis, schema::PORTE_SUR).await? {
        graphe::delier(ctx, &voisin.edge_id).await?;
    }

    // L'arete depuis le client, elle, se recompose : son identifiant suit la
    // convention d'ecriture de `schema::arete`.
    let noeud_client = schema::noeud(schema::LABEL_CLIENT, client_id);
    graphe::delier(
        ctx,
        &schema::arete(schema::A_DEMANDE, &noeud_client, &noeud_devis),
    )
    .await

    // Le noeud `devis:<id>` subsiste : aucune RPC ne supprime un noeud.
}
