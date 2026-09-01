//! Catalogue et stock : ecritures documentaires simples, et interrogation.
//!
//! Ce module couvre les quatre facons de lire des documents, et surtout
//! quand choisir laquelle :
//!
//! | Besoin | Methode | Cout de `total_count` |
//! |---|---|---|
//! | Tout l'inventaire | `list_documents` | gratuit (compteur tenu a jour) |
//! | Une reference exacte | `search_documents` | comptage d'index |
//! | Plusieurs criteres + tri | `query_documents` | proportionnel aux candidats |
//! | Un article precis | `get_document` | sans objet |

use crate::contexte::Contexte;
use crate::erreur::{ErreurApp, Resultat};
use crate::modele::{Article, MouvementStock, SensMouvement};
use crate::pagination::{TAILLE_PAGE, collecter_tout};
use crate::schema;
use rocia_db_sdk::{
    CollectionInfo, DocumentQueryFilter, DocumentQueryOperator, DocumentQuerySort,
    DocumentQuerySortDirection,
};
use serde_json::json;

/// Importe un lot d'articles, avec des cles d'idempotence stables.
///
/// C'est le cas d'usage type de `put_document_with_request_id` : un import
/// interrompu au milieu se relance a l'identique, et le serveur reconnait les
/// ecritures deja passees sur `(tenant, operation, request_id)` au lieu de
/// les rejouer.
///
/// `put_document` n'ecrit que le document — aucun noeud de graphe. Les noeuds
/// d'article sont ecrits separement, enrichis, par
/// [`crate::graphe::enregistrer_articles`].
pub async fn importer_articles(ctx: &Contexte, articles: &[Article]) -> Resultat<()> {
    for article in articles {
        ctx.client
            .put_document_with_request_id(
                &ctx.tenant,
                schema::ARTICLES,
                &article.id,
                article,
                ctx.cle(&format!("import-article:{}", article.id)),
            )
            .await?;
    }
    Ok(())
}

/// Ecrit un article en place, sans cle d'idempotence maitrisee : le SDK en
/// genere une. C'est le bon choix pour une mise a jour ponctuelle, ou rejouer
/// deux fois la meme ecriture est sans consequence.
pub async fn enregistrer_article(ctx: &Contexte, article: &Article) -> Resultat<()> {
    ctx.client
        .put_document(&ctx.tenant, schema::ARTICLES, &article.id, article)
        .await?;
    Ok(())
}

pub async fn lire_article(ctx: &Contexte, id: &str) -> Resultat<Article> {
    Ok(ctx
        .client
        .get_document::<Article>(&ctx.tenant, schema::ARTICLES, id)
        .await?)
}

/// Tout l'inventaire, page apres page.
pub async fn inventaire(ctx: &Contexte) -> Resultat<Vec<Article>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_documents::<Article>(tenant, schema::ARTICLES, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

/// Retrouve un article par sa reference commerciale (egalite exacte).
pub async fn chercher_par_reference(ctx: &Contexte, reference: &str) -> Resultat<Vec<Article>> {
    let page = ctx
        .client
        .search_documents::<Article>(
            &ctx.tenant,
            schema::ARTICLES,
            "reference",
            &reference,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok(page.items)
}

/// Recherche catalogue : une famille, un mot dans la designation, trie.
///
/// Trois regles de `query_documents` valent d'etre connues :
///
/// - les filtres se combinent en ET ;
/// - `Contains` est une sous-chaine insensible a la casse, mais un terme de
///   moins de 3 caracteres n'est pas indexable : une requete dont *aucun*
///   filtre n'est indexable part en `INVALID_ARGUMENT` plutot que de declencher
///   un balayage complet. D'ou le `Eq` sur `famille` qui accompagne ici le
///   `Contains` ;
/// - le tri final est toujours departage par identifiant de document, donc
///   l'ordre reste total et stable d'une page a l'autre.
pub async fn rechercher(
    ctx: &Contexte,
    famille: &str,
    mot_cle: &str,
) -> Resultat<(Vec<Article>, u64)> {
    let filtres = [
        DocumentQueryFilter {
            field: "famille".to_string(),
            operator: DocumentQueryOperator::Eq,
            values: vec![json!(famille)],
        },
        DocumentQueryFilter {
            field: "designation".to_string(),
            operator: DocumentQueryOperator::Contains,
            values: vec![json!(mot_cle)],
        },
    ];
    let tri = [DocumentQuerySort {
        field: "designation".to_string(),
        direction: DocumentQuerySortDirection::Asc,
    }];

    let page = ctx
        .client
        .query_documents::<Article>(
            &ctx.tenant,
            schema::ARTICLES,
            &filtres,
            &tri,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok((page.items, page.total_count))
}

/// Les articles a reapprovisionner.
///
/// Les operateurs disponibles sont `Eq`, `In` et `Contains` : il n'y a pas de
/// comparaison entre deux champs, donc « stock < stock_mini » ne se traduit
/// pas en filtre. La requete ramene les articles actifs tries par stock
/// croissant, et la comparaison se fait ici — sur un jeu deja restreint et
/// deja ordonne par le serveur.
pub async fn a_reapprovisionner(ctx: &Contexte) -> Resultat<Vec<Article>> {
    let filtres = [DocumentQueryFilter {
        field: "actif".to_string(),
        operator: DocumentQueryOperator::Eq,
        values: vec![json!(true)],
    }];
    let tri = [DocumentQuerySort {
        field: "stock".to_string(),
        direction: DocumentQuerySortDirection::Asc,
    }];

    let page = ctx
        .client
        .query_documents::<Article>(
            &ctx.tenant,
            schema::ARTICLES,
            &filtres,
            &tri,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok(page
        .items
        .into_iter()
        .filter(Article::sous_le_seuil)
        .collect())
}

/// Les articles d'une liste de familles (`In` sur un champ scalaire).
pub async fn par_familles(ctx: &Contexte, familles: &[&str]) -> Resultat<Vec<Article>> {
    let filtres = [DocumentQueryFilter {
        field: "famille".to_string(),
        operator: DocumentQueryOperator::In,
        values: familles.iter().map(|famille| json!(famille)).collect(),
    }];
    let tri = [DocumentQuerySort {
        field: "prix_unitaire_ht".to_string(),
        direction: DocumentQuerySortDirection::Desc,
    }];

    let page = ctx
        .client
        .query_documents::<Article>(
            &ctx.tenant,
            schema::ARTICLES,
            &filtres,
            &tri,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok(page.items)
}

/// Les collections ecrites pour ce tenant, avec le nombre de documents.
///
/// Aucune collection ne se declare : elle apparait au premier document ecrit.
/// `list_collections` est donc la seule facon de savoir ce qui existe
/// vraiment cote serveur.
pub async fn collections(ctx: &Contexte) -> Resultat<Vec<CollectionInfo>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_collections(tenant, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

/// Enregistre un mouvement de stock et met l'article a jour.
///
/// Deux ecritures, pas une transaction : RociaDB n'offre pas d'atomicite
/// entre deux documents. Le mouvement est ecrit *apres* la mise a jour du
/// stock, pour qu'une panne entre les deux laisse un stock a jour sans sa
/// trace, plutot qu'une trace sans effet — le cas le plus facile a rattraper.
pub async fn mouvementer(
    ctx: &Contexte,
    article_id: &str,
    sens: SensMouvement,
    quantite: i64,
    origine: &str,
) -> Resultat<Article> {
    if quantite <= 0 {
        return Err(ErreurApp::metier(format!(
            "quantite invalide ({quantite}) pour le mouvement sur {article_id}"
        )));
    }

    let mut article = lire_article(ctx, article_id).await?;
    let nouveau_stock = article.stock + sens.signe() * quantite;
    if nouveau_stock < 0 {
        return Err(ErreurApp::metier(format!(
            "stock insuffisant sur {article_id} : {} disponible(s), {quantite} demande(s)",
            article.stock
        )));
    }

    article.stock = nouveau_stock;
    enregistrer_article(ctx, &article).await?;

    let maintenant = chrono::Utc::now();
    let horodatage = maintenant.to_rfc3339();
    let mouvement = MouvementStock {
        id: format!("MVT-{article_id}-{}", maintenant.format("%Y%m%dT%H%M%S%6f")),
        article_id: article_id.to_string(),
        sens,
        quantite,
        origine: origine.to_string(),
        horodatage,
    };
    ctx.client
        .put_document(
            &ctx.tenant,
            schema::MOUVEMENTS_STOCK,
            &mouvement.id,
            &mouvement,
        )
        .await?;

    Ok(article)
}

/// L'historique des mouvements d'un article.
pub async fn mouvements(ctx: &Contexte, article_id: &str) -> Resultat<Vec<MouvementStock>> {
    let page = ctx
        .client
        .search_documents::<MouvementStock>(
            &ctx.tenant,
            schema::MOUVEMENTS_STOCK,
            "article_id",
            &article_id,
            Some(TAILLE_PAGE),
            None,
        )
        .await?;
    Ok(page.items)
}

/// L'inventaire au format CSV, pret a etre televerse.
pub fn exporter_csv(articles: &[Article]) -> String {
    let mut csv = String::from("reference;designation;famille;stock;stock_mini;prix_ht_centimes\n");
    for article in articles {
        csv.push_str(&format!(
            "{};{};{};{};{};{}\n",
            article.reference,
            article.designation,
            article.famille,
            article.stock,
            article.stock_mini,
            article.prix_unitaire_ht,
        ));
    }
    csv
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::modele::TVA_NORMALE;

    fn article(reference: &str, stock: i64) -> Article {
        Article {
            id: format!("ART-{reference}"),
            reference: reference.to_string(),
            designation: format!("Article {reference}"),
            famille: "quincaillerie".to_string(),
            prix_unitaire_ht: 1250,
            taux_tva: TVA_NORMALE,
            stock,
            stock_mini: 5,
            actif: true,
        }
    }

    #[test]
    fn l_export_csv_porte_un_entete_et_une_ligne_par_article() {
        let csv = exporter_csv(&[article("VIS", 10), article("ECROU", 3)]);
        let lignes: Vec<&str> = csv.lines().collect();

        assert_eq!(lignes.len(), 3, "un entete + deux articles");
        assert!(lignes[0].starts_with("reference;"));
        assert_eq!(lignes[1], "VIS;Article VIS;quincaillerie;10;5;1250");
        assert_eq!(lignes[2], "ECROU;Article ECROU;quincaillerie;3;5;1250");
    }

    #[test]
    fn l_export_d_un_inventaire_vide_garde_son_entete() {
        // Un fichier vide reste un fichier valide cote RociaDB (un seul
        // message de metadonnees, sans donnee), mais un CSV sans entete
        // casserait le tableur qui l'ouvre.
        let csv = exporter_csv(&[]);
        assert_eq!(csv.lines().count(), 1);
    }
}
