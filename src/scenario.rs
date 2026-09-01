//! Le scenario de bout en bout : une commande, de l'appel d'offres au
//! reglement, en passant par toutes les fonctionnalites du SDK.

use crate::contexte::Contexte;
use crate::erreur::Resultat;
use crate::modele::{Ligne, SensMouvement, formater};
use crate::pagination::TAILLE_PAGE;
use crate::schema;
use crate::{admin, catalogue, graphe, jeu_donnees, nettoyage, pieces, tiers, ventes};

/// Titre de section, pour que la sortie se lise comme un deroule.
fn etape(numero: u8, titre: &str) {
    println!("\n\x1b[1m{numero}. {titre}\x1b[0m");
}

fn ligne_resultat(texte: impl AsRef<str>) {
    println!("   {}", texte.as_ref());
}

/// Deroule tout : tiers, catalogue, graphe, vente, facture, pieces jointes,
/// exploration, puis (optionnellement) la purge.
pub async fn jouer(ctx: &Contexte, purger: bool) -> Resultat<()> {
    println!(
        "Tenant « {} », graphe « {} », bucket « {} », job d'idempotence « {} »",
        ctx.tenant, ctx.graphe, ctx.bucket, ctx.job
    );

    semer(ctx).await?;
    interroger_le_catalogue(ctx).await?;
    vendre(ctx).await?;
    joindre_les_pieces(ctx).await?;
    traverser_le_graphe(ctx).await?;
    explorer(ctx).await?;

    if purger {
        etape(7, "Purge des donnees de demonstration");
        let bilan = nettoyage::purger(ctx).await?;
        ligne_resultat(format!(
            "{} document(s), {} arete(s) et {} fichier(s) supprimes",
            bilan.documents, bilan.aretes, bilan.fichiers
        ));
        ligne_resultat(format!(
            "{} noeud(s) subsistent : aucune RPC ne supprime un noeud",
            bilan.noeuds_orphelins
        ));
    }

    println!("\n\x1b[1mTermine.\x1b[0m");
    Ok(())
}

/// Etape 1 : les tiers, le catalogue, et le graphe d'approvisionnement.
pub async fn semer(ctx: &Contexte) -> Resultat<()> {
    etape(1, "Tiers, catalogue et approvisionnements");

    for client in jeu_donnees::clients() {
        tiers::creer_client(ctx, &client).await?;
    }
    for fournisseur in jeu_donnees::fournisseurs() {
        tiers::creer_fournisseur(ctx, &fournisseur).await?;
    }
    ligne_resultat(format!(
        "{} client(s) et {} fournisseur(s) ecrits, chacun avec son noeud de graphe",
        jeu_donnees::clients().len(),
        jeu_donnees::fournisseurs().len()
    ));

    let articles = jeu_donnees::articles();
    catalogue::importer_articles(ctx, &articles).await?;
    // Les noeuds d'articles sont ecrits a part, enrichis : une traversee
    // affichera la designation sans relire le document.
    graphe::enregistrer_articles(ctx, &articles).await?;
    ligne_resultat(format!("{} article(s) importes", articles.len()));

    // Les aretes en dernier : leurs deux extremites doivent deja exister.
    for (fournisseur_id, catalogue_fournisseur) in jeu_donnees::approvisionnements() {
        graphe::lier_fournisseur_aux_articles(ctx, fournisseur_id, &catalogue_fournisseur).await?;
    }
    ligne_resultat("Aretes « fournit » posees");

    // Un article ajoute hors import : `put_document` ecrit le document,
    // `put_node` pose son noeud. C'est, en deux appels explicites, ce que
    // `create_document` fait en un seul.
    let nouveau = crate::modele::Article {
        id: "ART-006".to_string(),
        reference: "COLLE-PU".to_string(),
        designation: "Colle polyurethane 310 ml".to_string(),
        famille: "quincaillerie".to_string(),
        prix_unitaire_ht: 890,
        taux_tva: crate::modele::TVA_NORMALE,
        stock: 24,
        stock_mini: 10,
        actif: true,
    };
    catalogue::enregistrer_article(ctx, &nouveau).await?;
    graphe::enregistrer_noeud(
        ctx,
        &schema::noeud(schema::LABEL_ARTICLE, &nouveau.id),
        &schema::RefDocument {
            collection: schema::ARTICLES.to_string(),
            id: nouveau.id.clone(),
        },
    )
    .await?;
    ligne_resultat(format!(
        "Article {} ajoute hors import (put_document + put_node)",
        nouveau.reference
    ));

    // Une reception fournisseur : le mouvement de stock d'entree.
    let article =
        catalogue::mouvementer(ctx, "ART-005", SensMouvement::Entree, 50, "reception F-001")
            .await?;
    ligne_resultat(format!(
        "Reception de 50 « {} » : stock a {}",
        article.designation, article.stock
    ));

    let repares = tiers::reparer_liaisons(ctx).await?;
    ligne_resultat(format!(
        "Controle des liaisons document/noeud : {repares} reparation(s)"
    ));
    Ok(())
}

/// Etape 2 : les quatre facons de lire des documents.
pub async fn interroger_le_catalogue(ctx: &Contexte) -> Resultat<()> {
    etape(2, "Interrogation du catalogue");

    let inventaire = catalogue::inventaire(ctx).await?;
    let valeur: i64 = inventaire
        .iter()
        .map(crate::modele::Article::valeur_stock)
        .sum();
    ligne_resultat(format!(
        "list_documents : {} article(s), stock valorise a {}",
        inventaire.len(),
        formater(valeur)
    ));

    let trouves = catalogue::chercher_par_reference(ctx, "VIS-4X30").await?;
    ligne_resultat(format!(
        "search_documents « VIS-4X30 » : {}",
        trouves
            .first()
            .map_or("aucun resultat".to_string(), |article| article
                .designation
                .clone())
    ));

    let (resultats, total) = catalogue::rechercher(ctx, "quincaillerie", "vis").await?;
    ligne_resultat(format!(
        "query_documents (famille = quincaillerie ET designation contient « vis ») : \
         {} resultat(s) sur {total}",
        resultats.len()
    ));
    for article in &resultats {
        ligne_resultat(format!(
            "  - {} — {}",
            article.designation,
            formater(article.prix_unitaire_ht)
        ));
    }

    let chers = catalogue::par_familles(ctx, &["outillage", "documentation"]).await?;
    ligne_resultat(format!(
        "query_documents (famille In [outillage, documentation], prix decroissant) : {}",
        chers
            .iter()
            .map(|article| article.reference.clone())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let a_commander = catalogue::a_reapprovisionner(ctx).await?;
    ligne_resultat(format!(
        "Sous le seuil de reapprovisionnement : {}",
        if a_commander.is_empty() {
            "aucun".to_string()
        } else {
            a_commander
                .iter()
                .map(|article| {
                    format!(
                        "{} ({}/{})",
                        article.reference, article.stock, article.stock_mini
                    )
                })
                .collect::<Vec<_>>()
                .join(", ")
        }
    ));

    let email = "contact@menuiserie-bertrand.example";
    let clients = tiers::chercher_client_par_email(ctx, email).await?;
    ligne_resultat(format!(
        "search_documents sur les clients « {email} » : {}",
        clients
            .first()
            .map_or("aucun resultat".to_string(), |client| client
                .raison_sociale
                .clone())
    ));

    let par_siret = tiers::chercher_fournisseur_par_siret(ctx, "77889900100013").await?;
    let fournisseur = tiers::lire_fournisseur(ctx, "F-001").await?;
    ligne_resultat(format!(
        "search_documents sur les fournisseurs (SIRET) : {} resultat(s) ; \
         get_document F-001 : « {} », {} jours de delai",
        par_siret.len(),
        fournisseur.raison_sociale,
        fournisseur.delai_jours
    ));

    // Un identifiant absent rend `NOT_FOUND`, que `reason()` permet de
    // distinguer d'une vraie panne — ici, sans en faire une erreur.
    ligne_resultat(
        match tiers::lire_client_optionnel(ctx, "C-INEXISTANT").await? {
            Some(client) => format!("get_document C-INEXISTANT : {}", client.raison_sociale),
            None => "get_document C-INEXISTANT : NOT_FOUND, traite comme une absence".to_string(),
        },
    );
    Ok(())
}

/// Etape 3 : devis -> commande -> expedition -> facture -> reglement.
pub async fn vendre(ctx: &Contexte) -> Resultat<()> {
    etape(3, "Devis, commande, facture");

    let client = tiers::lire_client(ctx, "C-001").await?;
    let vis = catalogue::lire_article(ctx, "ART-001").await?;
    let equerre = catalogue::lire_article(ctx, "ART-003").await?;

    let lignes = vec![
        Ligne {
            article_id: vis.id.clone(),
            designation: vis.designation.clone(),
            quantite: 12,
            prix_unitaire_ht: vis.prix_unitaire_ht,
            taux_tva: vis.taux_tva,
        },
        Ligne {
            article_id: equerre.id.clone(),
            designation: equerre.designation.clone(),
            quantite: 80,
            prix_unitaire_ht: equerre.prix_unitaire_ht,
            taux_tva: equerre.taux_tva,
        },
    ];

    let devis = ventes::etablir_devis(ctx, jeu_donnees::DEVIS_ID, &client.id, lignes).await?;
    ligne_resultat(format!(
        "Devis {} pour « {} » : {} HT, {} TVA, {} TTC",
        devis.id,
        client.raison_sociale,
        formater(devis.totaux.total_ht),
        formater(devis.totaux.total_tva),
        formater(devis.totaux.total_ttc)
    ));

    let commande = ventes::accepter_devis(ctx, &devis.id, jeu_donnees::COMMANDE_ID).await?;
    ligne_resultat(format!(
        "Devis accepte -> commande {} ({})",
        commande.id,
        commande.statut.code()
    ));

    let commande = ventes::expedier(ctx, &commande.id).await?;
    ligne_resultat(format!(
        "Commande {} : {}",
        commande.id,
        commande.statut.code()
    ));
    for ligne in &commande.lignes {
        let article = catalogue::lire_article(ctx, &ligne.article_id).await?;
        ligne_resultat(format!(
            "  - {} : -{} => stock {}",
            article.reference, ligne.quantite, article.stock
        ));
    }

    let facture = ventes::facturer(ctx, &commande.id, jeu_donnees::FACTURE_ID).await?;
    ligne_resultat(format!(
        "Facture {} emise, echeance {}, {} TTC",
        facture.id,
        facture.date_echeance,
        formater(facture.totaux.total_ttc)
    ));

    let (a_relancer, total) = ventes::factures_a_relancer(ctx).await?;
    ligne_resultat(format!(
        "query_documents (statut In [emise, en_retard], echeance croissante) : \
         {} facture(s) sur {total}",
        a_relancer.len()
    ));

    let facture = ventes::encaisser(ctx, &facture.id).await?;
    ligne_resultat(format!(
        "Facture {} : {}",
        facture.id,
        facture.statut.code()
    ));

    let mouvements = catalogue::mouvements(ctx, "ART-001").await?;
    ligne_resultat(format!(
        "Mouvements de stock de ART-001 : {}",
        mouvements
            .iter()
            .map(|mouvement| format!("{}{}", mouvement.sens.code(), mouvement.quantite))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    // Un second devis, refuse par le client : il part, et l'arete qui le
    // reliait au client aussi.
    let perceuse = catalogue::lire_article(ctx, "ART-004").await?;
    let devis_refuse = ventes::etablir_devis(
        ctx,
        "DEV-2026-0002",
        "C-002",
        vec![Ligne {
            article_id: perceuse.id.clone(),
            designation: perceuse.designation.clone(),
            quantite: 2,
            prix_unitaire_ht: perceuse.prix_unitaire_ht,
            taux_tva: perceuse.taux_tva,
        }],
    )
    .await?;
    ventes::annuler_devis(ctx, &devis_refuse.id, "C-002").await?;
    ligne_resultat(format!(
        "Devis {} annule : document supprime (idempotent) puis arete supprimee (ne l'est pas)",
        devis_refuse.id
    ));

    // Radiation du client inactif du jeu de donnees.
    tiers::supprimer_client(ctx, "C-003").await?;
    ligne_resultat("Client C-003 (inactif) radie");
    Ok(())
}

/// Etape 4 : les trois modes de televersement, et les deux de lecture.
pub async fn joindre_les_pieces(ctx: &Contexte) -> Resultat<()> {
    etape(4, "Pieces jointes");

    // 4a. Le PDF de la facture : le fichier tient en memoire, `upload_file`
    //     s'occupe du decoupage et de l'empreinte.
    let facture = ventes::lire_facture(ctx, jeu_donnees::FACTURE_ID).await?;
    let corps: Vec<String> = facture
        .lignes
        .iter()
        .map(|ligne| {
            format!(
                "{} x {} = {}",
                ligne.quantite,
                ligne.designation,
                formater(ligne.total_ht())
            )
        })
        .chain(std::iter::once(format!(
            "Total TTC : {}",
            formater(facture.totaux.total_ttc)
        )))
        .collect();
    let pdf = pieces::pdf_minimal(&format!("Facture {}", facture.id), &corps);
    let fichier_pdf = schema::pdf_facture(&facture.id);
    let empreinte = pieces::televerser(ctx, &fichier_pdf, &pdf, "application/pdf").await?;
    ligne_resultat(format!(
        "upload_file : {fichier_pdf} ({} octets, sha256 {}…)",
        pdf.len(),
        empreinte
            .iter()
            .take(4)
            .map(|octet| format!("{octet:02x}"))
            .collect::<String>()
    ));

    // 4b. L'export d'inventaire : produit par morceaux, redecoupe par le SDK.
    let inventaire = catalogue::inventaire(ctx).await?;
    let csv = catalogue::exporter_csv(&inventaire);
    let fichier_csv = schema::export_stock(&chrono::Utc::now().date_naive().to_string());
    pieces::televerser_en_flux(ctx, &fichier_csv, csv.into_bytes(), "text/csv").await?;
    ligne_resultat(format!("upload_file_chunked : {fichier_csv}"));

    // 4c. Une note interne, message par message.
    let note = b"Note interne : verifier le stock ART-005 avant la prochaine commande.\n";
    pieces::televerser_brut(ctx, "notes/relance-stock.txt", note, "text/plain").await?;
    ligne_resultat("upload_file_stream : notes/relance-stock.txt");

    let metadonnees = pieces::metadonnees(ctx, &fichier_pdf).await?;
    ligne_resultat(format!(
        "stat_file : {} octets, {}, cree le {}",
        metadonnees.size_bytes, metadonnees.content_type, metadonnees.created_at
    ));

    let telecharge = pieces::telecharger(ctx, &fichier_pdf).await?;
    ligne_resultat(format!(
        "download_file : {} octets relus, identiques a l'original : {}",
        telecharge.len(),
        telecharge == pdf
    ));

    let octets = pieces::telecharger_en_flux(ctx, &fichier_csv).await?;
    ligne_resultat(format!("download_file_stream : {octets} octets parcourus"));

    let fichiers = pieces::lister_fichiers(ctx).await?;
    ligne_resultat(format!("list_files : {}", fichiers.join(", ")));

    pieces::supprimer_avec_cle(ctx, "notes/relance-stock.txt").await?;
    ligne_resultat("delete_file : note interne supprimee");
    Ok(())
}

/// Etape 5 : les traversees de graphe.
pub async fn traverser_le_graphe(ctx: &Contexte) -> Resultat<()> {
    etape(5, "Traversees de graphe");

    let approvisionnes = graphe::articles_du_fournisseur(ctx, "F-001").await?;
    ligne_resultat(format!(
        "get_outgoing_neighbor_nodes (F-001 -fournit->) : {}",
        approvisionnes
            .iter()
            .map(|voisin| voisin.value.designation.clone())
            .collect::<Vec<_>>()
            .join(" | ")
    ));

    let sources = graphe::fournisseurs_de_l_article(ctx, "ART-001").await?;
    ligne_resultat(format!(
        "get_incoming_neighbor_nodes (-fournit-> ART-001) : {}",
        sources
            .iter()
            .map(|voisin| voisin.value.id.clone())
            .collect::<Vec<_>>()
            .join(", ")
    ));

    let devis = ventes::devis_du_client(ctx, "C-001").await?;
    ligne_resultat(format!(
        "neighbors_out (C-001 -a_demande->) : {}",
        devis.join(", ")
    ));

    let commande = ventes::commande_de_la_facture(ctx, jeu_donnees::FACTURE_ID).await?;
    ligne_resultat(format!(
        "neighbors_in (-facture_par-> {}) : {}",
        jeu_donnees::FACTURE_ID,
        commande.unwrap_or_else(|| "aucune".to_string())
    ));

    // Le noeud pose par `create_document` : un pointeur vers le document.
    let node_id = schema::noeud(schema::LABEL_FACTURE, jeu_donnees::FACTURE_ID);
    let brut = graphe::noeud_brut(ctx, &node_id).await?;
    let reference = graphe::noeud_reference(ctx, &node_id).await?;
    ligne_resultat(format!("get_node (brut) : {brut}"));
    ligne_resultat(format!(
        "get_node_as (typé) : collection « {} », id « {} »",
        reference.collection, reference.id
    ));
    Ok(())
}

/// Etape 6 : ce que le deploiement contient, et l'etat du jeton.
pub async fn explorer(ctx: &Contexte) -> Resultat<()> {
    etape(6, "Exploration du deploiement");

    match admin::tenants(ctx).await? {
        Some(tenants) => ligne_resultat(format!("list_tenants : {}", tenants.join(", "))),
        None => ligne_resultat(
            "list_tenants : refuse (PERMISSION_DENIED) — cette RPC couvre tout le \
             deploiement et peut relever d'une politique dediee",
        ),
    }

    let collections = catalogue::collections(ctx).await?;
    ligne_resultat(format!(
        "list_collections : {}",
        collections
            .iter()
            .map(|info| format!("{} ({})", info.collection, info.count))
            .collect::<Vec<_>>()
            .join(", ")
    ));

    ligne_resultat(format!(
        "list_graphs : {}",
        graphe::lister_graphes(ctx).await?.join(", ")
    ));

    let noeuds = graphe::lister_noeuds(ctx).await?;
    ligne_resultat(format!(
        "list_nodes : {} noeud(s), dont {}",
        noeuds.len(),
        noeuds
            .iter()
            .take(3)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ")
    ));

    ligne_resultat(format!(
        "list_buckets : {}",
        admin::buckets(ctx).await?.join(", ")
    ));

    ligne_resultat(format!(
        "list_documents (total_count gratuit) : {} client(s)",
        tiers::compter_clients(ctx).await?
    ));

    // Le rejeu apres renouvellement du jeton, sur un appel reel : sans
    // authentification c'est un simple appel de plus, avec c'est la seule
    // reprise qui ait un sens sur `UNAUTHENTICATED`.
    let fournisseurs = admin::avec_reprise(ctx, || async {
        let page = ctx
            .client
            .list_documents::<serde_json::Value>(
                &ctx.tenant,
                schema::FOURNISSEURS,
                Some(TAILLE_PAGE),
                None,
            )
            .await?;
        Ok(page.total_count)
    })
    .await?;
    ligne_resultat(format!(
        "Appel protege par un rejeu apres renouvellement : {fournisseurs} fournisseur(s)"
    ));

    // Les deux faces du cycle de vie du jeton. Sans authentification, les
    // deux ne font rien et rendent `Ok` : le code appelant n'a pas a savoir
    // comment le client a ete construit.
    admin::rafraichir_jeton(ctx).await?;
    ligne_resultat("refresh_auth_token : renouvellement immediat, l'appelant attend");
    admin::invalider_jeton(ctx);
    ligne_resultat("invalidate_auth_token : marque le jeton suspect sans attendre");
    Ok(())
}
