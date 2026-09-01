//! Un ERP miniature — devis, bons de commande, factures, stock, clients,
//! fournisseurs — bati sur [`rocia_db_sdk`], le SDK Rust de RociaDB.
//!
//! L'exemple est volontairement petit cote metier, et exhaustif cote SDK :
//! chaque methode publique du SDK y apparait a l'endroit ou elle est le bon
//! outil, jamais pour la citation.
//!
//! # Ou trouver quoi
//!
//! | Service RociaDB | Module | Ce qu'il montre |
//! |---|---|---|
//! | Documents | [`tiers`], [`catalogue`], [`ventes`] | ecriture, lecture, recherche, requete, suppression |
//! | Graphe | [`graphe`] | noeuds et aretes, par lot, traversees dans les deux sens |
//! | Fichiers | [`pieces`] | les trois modes de televersement, les deux de lecture |
//! | Tenants | [`admin`] | enumeration du deploiement, cycle de vie du jeton |
//!
//! Les modules transverses : [`contexte`] (construction du client),
//! [`erreur`] (lecture d'une `RociaDbError`), [`pagination`] (parcours de
//! curseur), [`schema`] (conventions de nommage), [`modele`] (le metier, et
//! les seuls calculs testables sans serveur).
//!
//! # Trois regles qui expliquent la structure
//!
//! 1. **Rien ne se declare.** Une collection, un graphe ou un bucket existe
//!    des le premier ecrit. [`schema`] centralise donc tout le vocabulaire,
//!    parce qu'une faute de frappe y creerait silencieusement une collection
//!    de plus au lieu d'echouer.
//! 2. **Le document est la source de verite, le graphe est un index.** Aucune
//!    RPC ne relit la valeur d'une arete : ce qui doit etre relu vit dans le
//!    document.
//! 3. **Rien n'est atomique entre deux ecritures.** `create_document` ecrit le
//!    document puis le noeud sans transaction ; l'ordre des ecritures et les
//!    cles d'idempotence sont donc des choix, pas des details.

pub mod admin;
pub mod auth_avance;
pub mod catalogue;
pub mod contexte;
pub mod erreur;
pub mod graphe;
pub mod jeu_donnees;
pub mod modele;
pub mod nettoyage;
pub mod pagination;
pub mod pieces;
pub mod scenario;
pub mod schema;
pub mod tiers;
pub mod ventes;
