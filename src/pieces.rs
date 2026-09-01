//! Pieces jointes : le service fichier, dans ses trois modes de televersement.
//!
//! | Methode | Ce qu'elle fait pour vous | Quand la choisir |
//! |---|---|---|
//! | `upload_file` | decoupe, calcule le SHA-256, verifie la taille | le fichier tient en memoire |
//! | `upload_file_chunked` | redecoupe un flux, verifie le total annonce | la source est un flux, la taille et l'empreinte sont connues d'avance |
//! | `upload_file_stream` | rien | vous construisez chaque message vous-meme |
//!
//! Le contrat de fil, que les deux premieres respectent pour vous :
//! le **premier** message porte les metadonnees (`tenant_id`, `bucket`,
//! `file_id`, `size_bytes`, `content_type`, `checksum`, `request_id`) ; les
//! suivants ne sont lus que pour leur `chunk` ; aucun `chunk` ne depasse
//! 1 Mio ; la somme des `chunk` doit valoir exactement `size_bytes` ; et le
//! `checksum` fait exactement 32 octets — le serveur en verifie la longueur,
//! jamais le contenu.

use crate::contexte::Contexte;
use crate::erreur::{ErreurApp, Resultat};
use crate::pagination::{TAILLE_PAGE, collecter_tout};
use rocia_db_sdk::{FileStreamUploadOptions, FileUploadOptions, StatResponse, UploadRequest};
use sha2::{Digest, Sha256};

/// Taille de message du contrat de fil : le maximum accepte par le serveur,
/// donc le nombre minimal de messages pour un fichier donne.
const TAILLE_CHUNK: usize = 1024 * 1024;

/// Televerse un fichier deja en memoire.
///
/// Avec `checksum: None`, `upload_file` calcule le SHA-256 du tampon et
/// decoupe en messages de 1 Mio. On le fournit ici explicitement pour montrer
/// la contrainte : exactement 32 octets, sinon l'appel echoue cote client,
/// avant tout envoi.
pub async fn televerser(
    ctx: &Contexte,
    file_id: &str,
    contenu: &[u8],
    content_type: &str,
) -> Resultat<Vec<u8>> {
    let empreinte = Sha256::digest(contenu).to_vec();
    ctx.client
        .upload_file(
            &ctx.tenant,
            &ctx.bucket,
            file_id,
            contenu,
            FileUploadOptions {
                content_type: content_type.to_string(),
                checksum: Some(empreinte.clone()),
                request_id: Some(ctx.cle(&format!("piece:{file_id}"))),
            },
        )
        .await?;
    Ok(empreinte)
}

/// Televerse un contenu produit par morceaux, sans le retenir en entier.
///
/// `size_bytes` et `checksum` voyagent sur le tout premier message gRPC,
/// donc avant que la moindre donnee ait ete lue du flux : les deux doivent
/// etre connus d'avance. Si le flux finit par produire un nombre d'octets
/// different de celui annonce, l'echec est cote client
/// (`RociaDbError::Validation`), pas au bout de l'envoi.
///
/// Les morceaux fournis ici font 64 Kio : `upload_file_chunked` les
/// reagglomere en messages de 1 Mio, quelle que soit leur taille d'origine.
pub async fn televerser_en_flux(
    ctx: &Contexte,
    file_id: &str,
    contenu: Vec<u8>,
    content_type: &str,
) -> Resultat<()> {
    let taille = contenu.len() as u64;
    let empreinte = Sha256::digest(&contenu).to_vec();
    let morceaux: Vec<Vec<u8>> = contenu.chunks(64 * 1024).map(<[u8]>::to_vec).collect();

    ctx.client
        .upload_file_chunked(
            &ctx.tenant,
            &ctx.bucket,
            file_id,
            taille,
            empreinte,
            futures::stream::iter(morceaux),
            FileStreamUploadOptions {
                content_type: content_type.to_string(),
                request_id: Some(ctx.cle(&format!("export:{file_id}"))),
            },
        )
        .await?;
    Ok(())
}

/// Televerse en construisant chaque message protobuf a la main.
///
/// `upload_file_stream` est l'echappatoire bas niveau : aucun redecoupage,
/// aucun plafond applique, aucune empreinte calculee. Un `size_bytes` faux ou
/// une empreinte qui ne correspond pas aux octets passent sans bruit — le
/// serveur ne verifie que la *longueur* de l'empreinte. A n'utiliser que
/// lorsqu'on a besoin de maitriser le flux message par message.
pub async fn televerser_brut(
    ctx: &Contexte,
    file_id: &str,
    contenu: &[u8],
    content_type: &str,
) -> Resultat<()> {
    let messages = construire_messages(
        &ctx.tenant,
        &ctx.bucket,
        file_id,
        contenu,
        content_type,
        &ctx.cle(&format!("brut:{file_id}")),
    );
    ctx.client
        .upload_file_stream(futures::stream::iter(messages))
        .await?;
    Ok(())
}

/// Construit la suite de messages d'un televersement, en respectant le
/// contrat de fil. Fonction pure, donc testable sans serveur.
fn construire_messages(
    tenant: &str,
    bucket: &str,
    file_id: &str,
    contenu: &[u8],
    content_type: &str,
    request_id: &str,
) -> Vec<UploadRequest> {
    let entete = UploadRequest {
        tenant_id: tenant.to_string(),
        bucket: bucket.to_string(),
        file_id: file_id.to_string(),
        size_bytes: contenu.len() as u64,
        content_type: content_type.to_string(),
        checksum: Sha256::digest(contenu).to_vec(),
        // Un fichier vide est un cas valide : un seul message, sans donnee.
        chunk: contenu.iter().take(TAILLE_CHUNK).copied().collect(),
        request_id: request_id.to_string(),
    };

    // Le premier morceau voyage deja avec les metadonnees ; les messages
    // suivants ne portent que leur `chunk`.
    let mut messages = vec![entete];
    for morceau in contenu.chunks(TAILLE_CHUNK).skip(1) {
        messages.push(UploadRequest {
            chunk: morceau.to_vec(),
            ..Default::default()
        });
    }
    messages
}

/// Les metadonnees d'un fichier, sans le telecharger.
pub async fn metadonnees(ctx: &Contexte, file_id: &str) -> Resultat<StatResponse> {
    Ok(ctx
        .client
        .stat_file(&ctx.tenant, &ctx.bucket, file_id)
        .await?)
}

/// Telecharge un fichier en entier.
pub async fn telecharger(ctx: &Contexte, file_id: &str) -> Resultat<Vec<u8>> {
    Ok(ctx
        .client
        .download_file(&ctx.tenant, &ctx.bucket, file_id)
        .await?)
}

/// Telecharge en flux, sans jamais tenir le fichier entier en memoire.
///
/// C'est le seul endroit du projet ou une erreur arrive en `tonic::Status`
/// brut plutot qu'en `RociaDbError` : le flux est rendu tel quel par le SDK.
/// La fonction ne compte ici que les octets, mais c'est exactement la boucle
/// qu'on ecrirait pour verser vers un fichier ou un autre flux.
pub async fn telecharger_en_flux(ctx: &Contexte, file_id: &str) -> Resultat<u64> {
    let mut flux = ctx
        .client
        .download_file_stream(&ctx.tenant, &ctx.bucket, file_id)
        .await?;

    let mut octets = 0u64;
    while let Some(reponse) = flux
        .message()
        .await
        .map_err(|status| ErreurApp::Flux(status.to_string()))?
    {
        octets += reponse.chunk.len() as u64;
    }
    Ok(octets)
}

/// Les buckets du tenant.
pub async fn lister_buckets(ctx: &Contexte) -> Resultat<Vec<String>> {
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

/// Les fichiers d'un bucket.
pub async fn lister_fichiers(ctx: &Contexte) -> Resultat<Vec<String>> {
    let client = &ctx.client;
    let tenant = ctx.tenant.as_str();
    let bucket = ctx.bucket.as_str();
    collecter_tout(TAILLE_PAGE, move |limite, curseur| async move {
        let page = client
            .list_files(tenant, bucket, Some(limite), curseur.as_deref())
            .await?;
        Ok(page.into())
    })
    .await
}

/// Supprime un fichier. Comme `delete_document`, c'est idempotent.
pub async fn supprimer(ctx: &Contexte, file_id: &str) -> Resultat<()> {
    ctx.client
        .delete_file(&ctx.tenant, &ctx.bucket, file_id)
        .await?;
    Ok(())
}

/// Supprime un fichier avec une cle d'idempotence explicite.
pub async fn supprimer_avec_cle(ctx: &Contexte, file_id: &str) -> Resultat<()> {
    ctx.client
        .delete_file_with_request_id(
            &ctx.tenant,
            &ctx.bucket,
            file_id,
            ctx.cle(&format!("suppression-piece:{file_id}")),
        )
        .await?;
    Ok(())
}

/// Un PDF minimal mais valide, pour avoir une vraie piece jointe a televerser
/// sans embarquer de generateur de documents.
pub fn pdf_minimal(titre: &str, lignes: &[String]) -> Vec<u8> {
    let mut texte = format!("BT /F1 14 Tf 50 780 Td ({}) Tj ET\n", echapper_pdf(titre));
    for (index, ligne) in lignes.iter().enumerate() {
        let y = 750 - (index as i32) * 18;
        texte.push_str(&format!(
            "BT /F1 10 Tf 50 {y} Td ({}) Tj ET\n",
            echapper_pdf(ligne)
        ));
    }

    let mut pdf = String::from("%PDF-1.4\n");
    let mut decalages = Vec::new();
    // La table `xref` d'un PDF indexe chaque objet par son decalage en octets
    // depuis le debut du fichier : il se releve juste avant d'ecrire l'objet.
    fn objet(pdf: &mut String, decalages: &mut Vec<usize>, corps: &str) {
        decalages.push(pdf.len());
        pdf.push_str(corps);
    }

    objet(
        &mut pdf,
        &mut decalages,
        "1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n",
    );
    objet(
        &mut pdf,
        &mut decalages,
        "2 0 obj\n<< /Type /Pages /Kids [3 0 R] /Count 1 >>\nendobj\n",
    );
    objet(
        &mut pdf,
        &mut decalages,
        "3 0 obj\n<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
         /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>\nendobj\n",
    );
    objet(
        &mut pdf,
        &mut decalages,
        "4 0 obj\n<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>\nendobj\n",
    );
    objet(
        &mut pdf,
        &mut decalages,
        &format!(
            "5 0 obj\n<< /Length {} >>\nstream\n{texte}endstream\nendobj\n",
            texte.len()
        ),
    );

    let depart_xref = pdf.len();
    pdf.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        decalages.len() + 1
    ));
    for decalage in &decalages {
        pdf.push_str(&format!("{decalage:010} 00000 n \n"));
    }
    pdf.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{depart_xref}\n%%EOF\n",
        decalages.len() + 1
    ));
    pdf.into_bytes()
}

/// Neutralise les caracteres qui ferment une chaine PDF, et laisse tomber le
/// non-ASCII : la police Helvetica de base n'a pas d'encodage UTF-8, un octet
/// multi-octets y produirait des glyphes faux.
fn echapper_pdf(texte: &str) -> String {
    texte
        .chars()
        .filter(char::is_ascii)
        .map(|caractere| match caractere {
            '\\' => r"\\".to_string(),
            '(' => r"\(".to_string(),
            ')' => r"\)".to_string(),
            autre => autre.to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_premier_message_porte_les_metadonnees_et_le_premier_morceau() {
        let contenu = vec![7u8; 10];
        let messages = construire_messages(
            "t",
            "b",
            "f.bin",
            &contenu,
            "application/octet-stream",
            "r1",
        );

        assert_eq!(messages.len(), 1, "10 octets tiennent dans un seul message");
        let entete = &messages[0];
        assert_eq!(entete.tenant_id, "t");
        assert_eq!(entete.bucket, "b");
        assert_eq!(entete.file_id, "f.bin");
        assert_eq!(entete.size_bytes, 10);
        assert_eq!(entete.request_id, "r1");
        assert_eq!(entete.checksum.len(), 32, "le serveur exige 32 octets");
        assert_eq!(entete.chunk, contenu);
    }

    #[test]
    fn aucun_message_ne_depasse_le_plafond_et_le_total_est_exact() {
        // Deux megaoctets et un octet : trois messages, dont le dernier tres
        // court. Le serveur rejette tout `chunk` au-dela de 1 Mio, et refuse
        // l'envoi si la somme des morceaux ne vaut pas `size_bytes`.
        let contenu = vec![1u8; 2 * TAILLE_CHUNK + 1];
        let messages = construire_messages("t", "b", "gros.bin", &contenu, "text/plain", "r2");

        assert_eq!(messages.len(), 3);
        assert!(messages.iter().all(|m| m.chunk.len() <= TAILLE_CHUNK));
        let total: usize = messages.iter().map(|m| m.chunk.len()).sum();
        assert_eq!(total as u64, messages[0].size_bytes);
        assert_eq!(total, contenu.len());

        // Seul le premier message porte les metadonnees.
        assert!(messages[1].tenant_id.is_empty());
        assert!(messages[2].file_id.is_empty());
    }

    #[test]
    fn un_fichier_vide_tient_en_un_seul_message_sans_donnee() {
        let messages = construire_messages("t", "b", "vide.txt", &[], "text/plain", "r3");

        assert_eq!(messages.len(), 1);
        assert_eq!(messages[0].size_bytes, 0);
        assert!(messages[0].chunk.is_empty());
        assert_eq!(messages[0].checksum.len(), 32);
    }

    #[test]
    fn le_pdf_genere_a_bien_la_forme_d_un_pdf() {
        let pdf = pdf_minimal("Facture FAC-2026-0001", &["Total : 120,00 EUR".to_string()]);
        assert!(pdf.starts_with(b"%PDF-1.4"));
        assert!(pdf.ends_with(b"%%EOF\n"));
        assert!(String::from_utf8_lossy(&pdf).contains("FAC-2026-0001"));
    }

    #[test]
    fn les_parentheses_sont_echappees_dans_le_pdf() {
        // Une parenthese non echappee fermerait la chaine PDF et rendrait le
        // fichier illisible.
        let pdf = pdf_minimal("Facture (acompte)", &[]);
        let texte = String::from_utf8_lossy(&pdf);
        assert!(texte.contains(r"Facture \(acompte\)"), "{texte}");
    }
}
