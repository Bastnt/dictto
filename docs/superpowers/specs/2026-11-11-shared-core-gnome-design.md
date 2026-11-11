# Cœur partagé et Dictto natif pour GNOME : design

Date : 2026-11-11

## Intention

Dictto Native (Rust + Win32) est désormais la version officielle. Le cœur de
l'application est aujourd'hui partagé de façon implicite : `apps/native` inclut
les services de `apps/desktop/src-tauri/src/services` par `#[path]` et imite
l'API Tauri via un shim (`host.rs`, `use crate::host as tauri`). Les modules de
synthèse vocale vivent eux dans `apps/native/src/speech*`, mêlant logique
métier et API Windows.

Deux objectifs :

1. **Extraire le cœur** dans un crate explicite, indépendant de tout OS et de
   Tauri, utilisé à la fois par la version web (Tauri) et par les versions
   natives.
2. **Créer Dictto natif pour Linux GNOME**, avec exactement les mêmes
   fonctionnalités que Dictto Native Windows, pour l'usage quotidien du
   propriétaire sur Fedora (GNOME par défaut, donc Wayland uniquement).

Le critère directeur reste la **légèreté** : mémoire au repos, threads, taille
sur disque, rien de chargé qui ne serve pas.

macOS est prévu plus tard (pas de machine de test pour l'instant). Il ne fait
pas partie de ce travail, mais l'architecture du cœur doit permettre de brancher
une version AppKit sans retoucher le cœur.

## Objectifs

- Un crate `crates/dictto-core` sans dépendance Windows, Tauri ni GUI, qui
  compile et passe ses tests sur Windows et Linux.
- Les deux variantes Windows (Tauri/web et Win32 natif) utilisent ce crate et
  se comportent exactement comme avant : même base SQLite, même keyring, mêmes
  réglages, mêmes raccourcis, garde de session croisée conservée.
- Une application `apps/gnome` offrant la parité fonctionnelle avec Dictto
  Native Windows :
  - dictée push-to-talk (Ctrl+Super par défaut, équivalent de Ctrl+Win), file
    de enregistrements, nouvel essai par appui court, transcription, nettoyage
    LLM, collage au curseur avec restauration du presse-papiers ;
  - overlay en bas d'écran (forme d'onde, spinner de traitement, bouton stop /
    annuler des échantillons) ;
  - icône de barre avec le même menu que le tray Windows ;
  - fenêtre de réglages avec les mêmes onglets que la version native (Général,
    Audio, API, Speech, Shortcuts, modèles) ;
  - les cinq raccourcis de synthèse (échantillon micro, échantillon lecture
    système, répéter ma voix clonée, lire en voix clonée, lire en voix moteur),
    lecture de la sélection avec repli sur le presse-papiers, cache des six
    derniers WAV, dossier audio ;
  - envoi de la synthèse vers un **micro virtuel** utilisable par Discord, avec
    restauration de l'entrée précédente et journal de récupération ;
  - sons d'interface, démarrage automatique, sélection et surveillance des
    périphériques, instance unique, `--smoke-test`.

## Non-objectifs

- macOS (seulement : ne rien mettre dans le cœur qui l'empêche).
- Session X11, KDE ou autres environnements que GNOME.
- Flatpak, RPM, publication sur extensions.gnome.org. Un script d'installation
  local suffit.
- Changer le comportement ou l'apparence des versions Windows.
- Faire tourner la version Tauri/web sous Linux.

## Anti-objectifs (échec même si « ça marche »)

- Une régression Windows : comportement, mémoire au repos, taille de
  l'exécutable natif, compatibilité de la base ou des réglages existants.
- Un démon Linux qui charge GTK, un moteur de rendu ou un interpréteur en
  permanence alors qu'il ne fait qu'attendre une touche.
- De la logique métier dans l'extension GNOME Shell : elle affiche et relaie,
  rien d'autre.
- Un cœur qui garde des `cfg(windows)` ou des codes de touches virtuelles
  Windows dans sa logique.

## Contraintes

- Rust, `tokio`, `async-trait`, `thiserror` + `Serialize` comme aujourd'hui.
- Conventions de commit du dépôt (`sujet: description`, commits de granularité
  moyenne sur `main`).
- Le développement se fait sur Windows ; la version GNOME ne peut être compilée
  et testée que sur la machine Fedora du propriétaire (ou en CI Linux). Le cœur
  doit donc avoir des tests qui tournent partout, et les tests propres à Linux
  doivent pouvoir tourner sans session graphique quand c'est possible.
- La base, le keyring et les réglages restent compatibles : les chaînes de
  raccourcis stockées (`"CTRL+WIN"`, `"CTRL+SHIFT+F8"`…) gardent leur format.

## Architecture

### Découpage des crates

```
crates/
  dictto-core/       logique neutre, testable partout
  dictto-windows/    implémentations Windows partagées par Tauri et Win32
apps/
  desktop/           Tauri/web (Windows), dépend de core + windows
  native/            Win32, dépend de core + windows
  gnome/             démon Rust + réglages GTK4 + extension GNOME Shell
```

`dictto-windows` existe parce que les deux variantes Windows partagent
aujourd'hui le même code WASAPI, hook clavier, injecteur, etc. Le mettre dans
le cœur derrière des `cfg` violerait l'anti-objectif ; le dupliquer dans les
deux apps créerait une dérive.

### Ce qui va dans `dictto-core`

Tout ce qui ne parle pas à l'OS :

- base SQLite, réglages, migrations, stockage des clés (le crate `keyring`
  avec le backend sélectionné par l'app hôte via features) ;
- transcription (`TranscriptionEngine`, OpenAI/Canary), LLM (`LLMProcessor`,
  prompts), tests d'endpoint, warmup ;
- orchestration du pipeline de dictée et machine à états des raccourcis :
  file d'enregistrements, appui court = nouvel essai, sessions de synthèse ;
- modèle de raccourcis : parsing des chaînes stockées vers un **type de touche
  neutre** (`Key::Ctrl`, `Key::Super`, `Key::F(8)`, `Key::Char('A')`…) et le
  `Tracker` de chord. Chaque plateforme traduit ses codes natifs (VK Windows,
  keycodes evdev) vers ce type ;
- synthèse vocale : `speech_api` (découverte, requêtes), découpage en
  passages, cache LRU des six WAV, préchargement du passage suivant, logique
  de rendu/rejouage indépendante de la sortie audio ;
- logique du routage micro virtuel : journal de l'entrée précédente,
  règles « restaurer seulement si notre endpoint est encore sélectionné »,
  « un changement manuel est prioritaire », délai d'attente de huit secondes
  avant le premier échantillon ;
- instance/garde de session croisée.

### Interface entre le cœur et une plateforme

Le shim `host.rs` (qui imite Tauri) est remplacé par des traits explicites dans
le cœur. L'app hôte fournit leurs implémentations à la construction. Liste
indicative ; le découpage exact est laissé au builder, tant que chaque trait
correspond à une vraie différence d'OS :

- `EventSink` : événements vers l'interface (`recording-state-changed`,
  `waveform-data`, état de synthèse…). Tauri les émet vers le WebView, Win32
  les poste en messages, GNOME les envoie en signaux D-Bus.
- `KeyboardSource` : flux d'événements clavier (touche neutre, appui/relâché),
  avec possibilité de supprimer l'événement quand un chord est consommé si la
  plateforme le permet.
- `AudioCapture` : micro sélectionné et lecture système (loopback), vers des
  échantillons PCM ; le cœur produit le WAV 16 kHz mono.
- `AudioOutput` : lecture d'un WAV vers la sortie par défaut ou vers le micro
  virtuel.
- `DeviceCatalog` : liste et surveillance des entrées, notification de
  disparition.
- `TextInjector` : poser le texte, coller, restaurer le presse-papiers.
- `SelectionReader` : texte sélectionné dans l'app au premier plan, repli
  presse-papiers.
- `VirtualMicRoute` : basculer/restaurer l'entrée par défaut des apps de
  communication vers le micro Dictto.
- `Autostart`, `Sounds`.

La version Tauri garde ses commandes `#[tauri::command]` dans `apps/desktop`,
comme fines enveloppes autour du cœur. Le feature `tauri-host` disparaît du
code partagé.

### Dictto GNOME

Trois pièces :

**1. Démon `dictto-gnome` (Rust, toujours actif).** Possède le cœur, l'audio,
le clavier, le service D-Bus `com.dictto.App` (nom unique = instance unique).
Ne charge aucune bibliothèque graphique.

- *Clavier* : lecture directe des périphériques `/dev/input/event*` (evdev).
  Mon choix plutôt que le portail GlobalShortcuts : le portail ne garantit pas
  un raccourci uniquement en modificateurs (Ctrl+Super), ne fournit pas les
  événements touche par touche dont le `Tracker` a besoin, et chaque raccourci
  passerait par une boîte de dialogue GNOME. Coût : l'utilisateur doit être
  dans le groupe `input`. Les événements ne peuvent pas être supprimés (sous
  Windows le hook peut avaler la touche) ; à vérifier que cela ne gêne pas avec
  les raccourcis par défaut, sinon le documenter.
  Gérer le branchement/débranchement de claviers.
- *Collage* : clavier virtuel `uinput` qui envoie Ctrl+V. Coût : une règle
  udev donnant l'accès à `/dev/uinput`. Le presse-papiers est posé, lu et
  restauré via l'extension (voir plus bas), car GNOME ne laisse pas un client
  sans fenêtre manipuler le presse-papiers Wayland.
- *Lecture de la sélection* : AT-SPI (accessibilité, équivalent d'UI
  Automation) pour la sélection de l'app focalisée ; repli sur la sélection
  primaire puis le presse-papiers via l'extension. Ne jamais écrire dans le
  presse-papiers pour lire.
- *Audio* : PipeWire directement (crate `pipewire`), pour la capture micro, la
  capture du moniteur de la sortie par défaut (équivalent loopback), la
  lecture, les sons et la surveillance des périphériques. Mon choix : une seule
  pile, déjà présente sur Fedora, plutôt que cpal + PipeWire.
- *Micro virtuel* : le démon crée un nœud PipeWire « Dictto Microphone »
  (source virtuelle) quand la fonction est activée, et bascule la source par
  défaut via les métadonnées par défaut (comme `wpctl set-default`), puis la
  restaure selon les règles du cœur. Discord utilise « Default ». Pas de
  pilote, pas de VB-CABLE.
- *Keyring* : Secret Service (GNOME Keyring) via `keyring`.
- *Données* : `$XDG_DATA_HOME/com.dictto.app` (base, `voice-sample.wav`,
  `Speech Audio/`).
- *Démarrage automatique* : fichier `.desktop` dans `~/.config/autostart`.
- *Session* : gérer verrouillage/déconnexion comme sous Windows (restauration
  du micro) via logind/D-Bus.

**2. Extension GNOME Shell `dictto@dictto.app` (JavaScript, minimale).**

- Indicateur dans la barre du haut avec le même menu que le tray Windows
  (Réglages, Quitter, etc.).
- Overlay en bas au centre, non focalisable, ne prenant pas les clics hors de
  ses boutons : forme d'onde, spinner, boutons stop/annuler.
- Presse-papiers et sélection primaire (lire, poser, restaurer) pour le compte
  du démon.
- Tout passe par D-Bus : signaux du démon vers l'extension, méthodes de
  l'extension appelées par le démon. Si l'extension est absente ou désactivée,
  le démon fonctionne en mode dégradé et le signale (notification) plutôt que
  d'échouer en silence.
- `metadata.json` liste les versions GNOME supportées ; viser la version de
  Fedora installée et la précédente.

**3. Réglages `dictto-gnome --settings` (GTK4 + libadwaita, processus séparé).**

Lancé par le démon à la demande (menu de l'indicateur), il quitte à la
fermeture : GTK n'est jamais résident. Mêmes pages et même logique de
sauvegarde explicite que la version Win32. Il lit/écrit la base via le cœur et
demande au démon de recharger (méthode D-Bus) ; les actions en direct (test
d'endpoint, découverte des modèles de synthèse, ouverture du dossier audio)
peuvent tourner dans le processus des réglages. Un seul exécutable, deux modes.

**Installation : `scripts/install-gnome.sh`**, équivalent de
`build-native.ps1` : compile en release, installe le binaire dans
`~/.local/bin`, le `.desktop` et l'icône, l'extension dans
`~/.local/share/gnome-shell/extensions`, puis, avec `sudo` et une seule fois,
la règle udev `uinput` et l'ajout au groupe `input`. Il explique qu'une
reconnexion est nécessaire (groupe + chargement de l'extension sous Wayland).

## Ordre des travaux

1. **Extraction du cœur**, Windows seulement : créer `dictto-core` et
   `dictto-windows`, y déplacer le code, remplacer le shim. Critère :
   `cargo test` du cœur, des deux apps Windows, `--smoke-test` natif, et un
   essai réel de dictée + synthèse sur Windows. Mesurer mémoire au repos et
   taille de l'exe natif avant/après.
2. **Le cœur compile et teste sous Linux** (CI GitHub Actions Ubuntu ajoutée
   pour `dictto-core`).
3. **Démon GNOME** : clavier, audio, pipeline de dictée, D-Bus.
4. **Extension** : indicateur, overlay, presse-papiers. Premier jalon
   utilisable : dicter sous Fedora.
5. **Réglages GTK4**.
6. **Synthèse vocale, sélection AT-SPI, micro virtuel PipeWire**, sons,
   autostart, mode dégradé.

Les étapes 3 à 6 sont écrites sous Windows, puis validées et corrigées sur la
Fedora du propriétaire (voir « Décisions du propriétaire »).

## Mesures de succès

- Windows : tests verts, comportement identique, mémoire et taille non
  dégradées de façon mesurable.
- GNOME : chaque fonctionnalité de la liste des objectifs fonctionne sous
  Fedora/Wayland ; démon au repos sans GTK chargé, avec mémoire et threads
  mesurés et rapportés.

## Décisions du propriétaire

- **Micro virtuel Windows** : la référence est le routage par **VB-CABLE**
  (commit `4f6f8b4`). Le pilote WaveRT `drivers/dictto-mic` imposait de
  démarrer Windows en mode test ; il est conservé en archive, hors du cœur et
  de `dictto-windows`, sans maintenance.
- **Développement GNOME en deux temps** : l'application GNOME est entièrement
  écrite depuis ce poste Windows, sans pouvoir la lancer. Le propriétaire
  clonera ensuite le dépôt sur sa Fedora et poursuivra la mise au point avec
  Claude CLI. Conséquences :
  - tout ce qui peut être vérifié sous Windows l'est : tests du cœur, et
    une compilation complète et les tests sans affichage de `apps/gnome` dans
    la distribution WSL Ubuntu présente sur ce poste (avec les paquets de
    développement GTK4, libadwaita, PipeWire et D-Bus). Ubuntu n'est pas
    Fedora : cela prouve que le code compile, pas qu'il fonctionne sous GNOME ;
  - la logique Linux testable sans session graphique (traduction evdev vers
    touches neutres, règles de routage PipeWire, format D-Bus) est isolée et
    couverte par des tests unitaires ;
  - un document `apps/gnome/FEDORA-BRINGUP.md` liste les dépendances à
    installer (`dnf`), l'ordre de mise en route, les points non vérifiés et
    les vérifications manuelles par fonctionnalité, pour la session Fedora ;
  - la version de Fedora n'est pas connue : l'extension cible les versions
    GNOME 47 à 49 (modules ES), à ajuster sur place.

## Décisions prises par l'assistant (à vérifier par le propriétaire)

- **Source par défaut PipeWire** : il n'existe qu'une source par défaut sous
  PipeWire (pas de rôle « communications » séparé). Le démon bascule cette
  source par défaut, mais résout d'abord l'entrée « par défaut » de Dictto vers
  le nœud concret du micro réel, pour que sa propre capture ne capte jamais sa
  synthèse. Les autres apps sur « Default » suivent pendant la lecture, puis
  sont restaurées.
- **Ctrl+Super et la touche Activités** : GNOME n'ouvre normalement pas la vue
  Activités quand une autre touche est tenue avec Super ; le builder le vérifie.
  Si ça gêne, on change le raccourci par défaut de la version GNOME (même format
  de chaîne) et on le documente, sans toucher `overlay-key` depuis l'extension.
- **Garde de session sous Linux** : instance unique par utilisateur (nom D-Bus
  de session), et le démon n'écoute le clavier et ne colle que quand sa session
  logind est active (changement rapide d'utilisateur).
- **Tolérance Windows** : pas de hausse au-delà du bruit de mesure (environ 1 %
  de mémoire au repos, environ 50 Ko d'exécutable). Les chiffres avant/après
  sont rapportés. Préférer les génériques au `dyn` là où c'est sur le chemin
  chaud n'est pas requis ; l'indirection est négligeable ici.
- **Synthèse vocale et Tauri** : la version Tauri ne gagne pas la synthèse
  vocale. Le cœur l'isole derrière une feature `speech` que Tauri n'active pas.

## Laissé au builder

- Découpage exact des traits et des modules à l'intérieur des crates.
- Choix des crates (`zbus`, `evdev`, `pipewire`, `atspi`, `gtk4`/`libadwaita`)
  et de leurs versions.
- Format précis de l'interface D-Bus.
- Le rythme d'envoi de la forme d'onde vers l'extension (aligné sur les
  événements audio existants).
