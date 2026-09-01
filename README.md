# erp-rocia-db

Un ERP miniature — **devis, bons de commande, factures, stock, clients,
fournisseurs** — bâti sur [`rocia-db-sdk`](https://crates.io/crates/rocia-db-sdk),
le SDK Rust de RociaDB.

Le métier est volontairement petit ; la couverture du SDK est complète. Les
**44 méthodes publiques** du SDK apparaissent chacune à l'endroit où elle est
le bon outil pour un besoin réel de l'ERP, jamais pour la citation.

## Démarrer

```bash
# protoc est nécessaire : le build.rs du SDK compile les .proto embarqués.
sudo apt-get install -y protobuf-compiler     # ou : brew install protobuf

cargo run -- --sans-auth demo
```

Sans serveur RociaDB à l'écoute sur `127.0.0.1:50051`, la commande s'arrête
sur un message explicite plutôt que sur un `unwrap` — c'est le module
[`erreur`](src/erreur.rs) qui traduit chaque variante de `RociaDbError` en
conduite à tenir.

### Options

| Option | Défaut | Rôle |
|---|---|---|
| `--hote` (`ROCIA_HOST`) | `http://127.0.0.1:50051` | hôte et port, **sans chemin** |
| `--tenant` (`ROCIA_TENANT`) | `demo-erp` | partition métier visée |
| `--sans-auth` | — | `disable_auth()`, pour le développement local |
| `--token-url` / `--client-id` / `--client-secret` | variables d'env | identifiants OAuth2 explicites |
| `--delai-connexion` | `10` | secondes avant d'abandonner la connexion |
| `--job` | horodaté | fige le préfixe des clés d'idempotence |

Sans `--sans-auth` ni identifiants explicites, le builder lit lui-même
`AUTH_TOKEN_URL`, `AUTH_CLIENT_ID` et `AUTH_CLIENT_SECRET` — c'est son
comportement par défaut. Voir [`.env.example`](.env.example).

### Sous-commandes

`demo` enchaîne tout ; les autres reprennent une étape isolément et
supposent que `demo` ou `seed` a déjà tourné.

```bash
cargo run -- --sans-auth demo --nettoyer   # le scénario complet, puis la purge
cargo run -- --sans-auth seed              # tiers, catalogue, approvisionnements
cargo run -- --sans-auth catalogue         # les quatre façons de lire des documents
cargo run -- --sans-auth ventes            # devis -> commande -> facture -> règlement
cargo run -- --sans-auth graphe            # traversées
cargo run -- --sans-auth pieces            # pièces jointes
cargo run -- --sans-auth admin             # tenants, collections, graphes, buckets, jeton
cargo run -- --sans-auth nettoyer          # purge
cargo run -- auth                          # le module `auth` sans RociaDbClient
```

Pour voir chaque RPC passer :

```bash
RUST_LOG=erp_rocia_db=info,rocia_db_sdk=debug cargo run -- --sans-auth demo
```

## Le modèle métier

```
client ──a_demande──▶ devis ──converti_en──▶ bon_commande ──facture_par──▶ facture
                        │
                        └──porte_sur──▶ article ◀──fournit── fournisseur
```

Chaque pièce est un **document** (source de vérité : lignes, totaux, statut)
doublé d'un **nœud** de graphe (index de navigation). Les expéditions et les
réceptions écrivent des documents `mouvements_stock` et mettent le stock de
l'article à jour ; le PDF de la facture et l'export d'inventaire partent dans
le **service fichier**.

Les montants sont des entiers de centimes et les taux de TVA des points de
base : aucun flottant ne circule, donc aucun arrondi ne dépend de l'ordre des
opérations.

## Où trouver quelle fonctionnalité

| Module | Ce qu'il montre |
|---|---|
| [`schema`](src/schema.rs) | conventions de nommage — rien ne se déclare côté serveur, une faute de frappe crée une collection de plus |
| [`contexte`](src/contexte.rs) | `RociaDbBuilder` : hôte, délai, et les trois modes d'authentification |
| [`tiers`](src/tiers.rs) | `create_document` et sa liaison document → nœud, recherche par champ, réparation d'une liaison manquante |
| [`catalogue`](src/catalogue.rs) | `put_document`, `list_documents`, `search_documents`, `query_documents`, `list_collections`, mouvements de stock |
| [`ventes`](src/ventes.rs) | l'enchaînement devis → commande → facture, filtres `In` et tri serveur |
| [`graphe`](src/graphe.rs) | nœuds et arêtes, par lot et à l'unité, traversées dans les deux sens |
| [`pieces`](src/pieces.rs) | les trois modes de téléversement, les deux de téléchargement, le contrat de fil |
| [`admin`](src/admin.rs) | `list_tenants`, cycle de vie du jeton, rejeu après `UNAUTHENTICATED` |
| [`auth_avance`](src/auth_avance.rs) | `TokenManager`, `fetch_token`, intercepteurs — sans `RociaDbClient` |
| [`pagination`](src/pagination.rs) | le parcours de curseur, commun aux trois formes de page |
| [`erreur`](src/erreur.rs) | lecture d'une `RociaDbError` : `code`, `reason`, `status`, et les deux prédicats |
| [`nettoyage`](src/nettoyage.rs) | trois sémantiques de suppression qui ne se ressemblent pas |
| [`modele`](src/modele.rs) | le métier, et les seuls calculs testables sans serveur |

## Ce que l'exemple cherche à faire comprendre

**Rien ne se déclare.** Collection, graphe et bucket existent dès le premier
écrit. C'est pratique, et c'est pourquoi tout le vocabulaire est centralisé
dans [`schema`](src/schema.rs) : une faute de frappe n'échoue pas, elle crée
silencieusement une collection de plus.

**Le document est la source de vérité, le graphe est un index.** Aucune RPC
ne relit la valeur d'une arête — `neighbors_out` ne rend qu'un `node_id` et un
`edge_id`. Ce qui doit être relu vit donc dans le document. Les nœuds
d'articles sont écrits *enrichis* (`put_nodes`) pour qu'une traversée affiche
une désignation sans un seul `get_document` de plus.

**Rien n'est atomique entre deux écritures.** `create_document` écrit le
document puis le nœud, sans transaction : si le second appel échoue, le
document reste sans liaison — d'où `tiers::reparer_liaisons`. L'ordre des
écritures est un choix à chaque fois : `catalogue::mouvementer` met le stock à
jour *avant* d'écrire sa trace, parce qu'un stock à jour sans trace se
rattrape plus facilement que l'inverse.

**Les suppressions n'ont pas toutes la même sémantique.**
`delete_document` et `delete_file` sont idempotents ; `delete_edge` rend
`NOT_FOUND` sur une arête absente ; et **rien ne supprime un nœud** — il n'y a
pas de RPC pour cela, un nœud orphelin reste listé par `list_nodes`.

**Les clés d'idempotence sont un choix, pas un détail.** Le serveur déduplique
sur `(tenant, operation, request_id)` pendant 24 h. Une clé *stable* est ce
qu'il faut pour rejouer un import interrompu — mais si cette démonstration
réutilisait les mêmes clés à chaque lancement, un second `demo` après un
`nettoyer` ne réécrirait rien. D'où le compromis : clé stable *à l'intérieur*
d'une exécution, unique d'une exécution à l'autre, et `--job` pour figer le
préfixe et retrouver le vrai comportement d'un import rejouable.

**`tenant_id` est une partition métier, pas une frontière de sécurité.** Il
n'est déduit d'aucune identité : n'importe quel client authentifié peut
adresser n'importe quel tenant. C'est à l'application de décider qui a le
droit de toucher à quoi.

**Deux erreurs seulement méritent un rejeu.** `UNAUTHENTICATED` est temporaire
(jeton expiré : renouveler puis rejouer, c'est ce que fait
`admin::avec_reprise`) ; `PERMISSION_DENIED` est définitif (portée
insuffisante, rafraîchir n'y changera rien). Tout le reste se lit dans
`reason()`.

## Développement

```bash
PROTOC=/usr/bin/protoc cargo fmt --all -- --check
PROTOC=/usr/bin/protoc cargo clippy --all-targets --all-features -- -D warnings
PROTOC=/usr/bin/protoc cargo test
```

Les 27 tests unitaires sont **déterministes et sans serveur** : calculs de
TVA et de totaux, conventions d'identifiants, parcours de curseur, contrat de
fil du téléversement, validation côté client du builder. Le scénario de bout
en bout, lui, a besoin d'un RociaDB à l'écoute.

## Licence

Apache-2.0, comme le SDK.
