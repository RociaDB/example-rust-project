//! Parcours de curseur, commun aux trois formes de page du SDK.
//!
//! Le SDK rend trois structures nommees plutot qu'un tuple : `Page<T>`
//! (`items`, `next_cursor`), `DocumentPage<T>` qui y ajoute `total_count`, et
//! `NeighborPage` pour le graphe. Toutes suivent le meme contrat : le curseur
//! est opaque, se repasse tel quel, et vaut `None` quand il n'y a plus rien.
//!
//! [`collecter_tout`] factorise la boucle une fois pour toutes.

use crate::erreur::Resultat;
use rocia_db_sdk::{DocumentPage, Neighbor, NeighborPage, Page};

/// Taille de page par defaut de cet exemple. Le SDK utilise 20 quand on ne
/// precise rien ; le serveur plafonne a `limits.max_page_size` (200 par
/// defaut) et a toujours le dernier mot.
pub const TAILLE_PAGE: u32 = 50;

/// Une page, debarrassee de ce qui differe entre les trois formes.
pub struct PageSimple<T> {
    pub elements: Vec<T>,
    pub curseur_suivant: Option<String>,
}

impl<T> From<Page<T>> for PageSimple<T> {
    fn from(page: Page<T>) -> Self {
        Self {
            elements: page.items,
            curseur_suivant: page.next_cursor,
        }
    }
}

impl<T> From<DocumentPage<T>> for PageSimple<T> {
    fn from(page: DocumentPage<T>) -> Self {
        Self {
            elements: page.items,
            curseur_suivant: page.next_cursor,
        }
    }
}

impl From<NeighborPage> for PageSimple<Neighbor> {
    fn from(page: NeighborPage) -> Self {
        Self {
            elements: page.neighbors,
            curseur_suivant: page.next_cursor,
        }
    }
}

/// Ramene toutes les pages en suivant le curseur jusqu'au bout.
///
/// `charger` recoit la limite et le curseur courant, et rend une page. La
/// boucle s'arrete des que le serveur ne rend plus de curseur — ou qu'il rend
/// le meme qu'a l'appel precedent, garde-fou contre une boucle infinie sur un
/// serveur qui repeterait son curseur.
///
/// A n'utiliser que sur des volumes bornes : tout est accumule en memoire.
/// Une collection de factures se pagine, elle ne se collecte pas.
pub async fn collecter_tout<T, F, Fut>(taille_page: u32, mut charger: F) -> Resultat<Vec<T>>
where
    F: FnMut(u32, Option<String>) -> Fut,
    Fut: Future<Output = Resultat<PageSimple<T>>>,
{
    let mut tout = Vec::new();
    let mut curseur: Option<String> = None;
    loop {
        let page = charger(taille_page, curseur.clone()).await?;
        tout.extend(page.elements);
        match page.curseur_suivant {
            Some(suivant) if curseur.as_deref() != Some(suivant.as_str()) => {
                curseur = Some(suivant);
            }
            _ => return Ok(tout),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    #[tokio::test]
    async fn collecter_tout_suit_le_curseur_jusqu_au_bout() {
        let appels = Cell::new(0);
        let elements: Vec<i32> = collecter_tout(2, |limite, curseur| {
            assert_eq!(limite, 2);
            appels.set(appels.get() + 1);
            async move {
                Ok(match curseur.as_deref() {
                    None => PageSimple {
                        elements: vec![1, 2],
                        curseur_suivant: Some("p2".to_string()),
                    },
                    Some("p2") => PageSimple {
                        elements: vec![3],
                        curseur_suivant: None,
                    },
                    autre => panic!("curseur inattendu: {autre:?}"),
                })
            }
        })
        .await
        .expect("le parcours doit aboutir");

        assert_eq!(elements, vec![1, 2, 3]);
        assert_eq!(appels.get(), 2, "une requete par page, pas une de plus");
    }

    #[tokio::test]
    async fn un_curseur_repete_arrete_le_parcours() {
        // Sans ce garde-fou, un serveur qui repete son curseur ferait tourner
        // la boucle indefiniment en accumulant les memes elements.
        let appels = Cell::new(0);
        let elements: Vec<i32> = collecter_tout(2, |_, _| {
            appels.set(appels.get() + 1);
            async move {
                Ok(PageSimple {
                    elements: vec![7],
                    curseur_suivant: Some("bloque".to_string()),
                })
            }
        })
        .await
        .expect("le parcours doit s'arreter, pas echouer");

        assert_eq!(elements, vec![7, 7]);
        assert_eq!(appels.get(), 2);
    }

    #[tokio::test]
    async fn une_page_vide_sans_curseur_rend_une_liste_vide() {
        let elements: Vec<i32> = collecter_tout(TAILLE_PAGE, |_, _| async move {
            Ok(PageSimple {
                elements: Vec::new(),
                curseur_suivant: None,
            })
        })
        .await
        .expect("le parcours doit aboutir");

        assert!(elements.is_empty());
    }
}
