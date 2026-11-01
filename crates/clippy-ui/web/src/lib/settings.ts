export type FieldKind = 'number' | 'text' | 'select' | 'bool' | 'folder' | 'hotkey' | 'slider' | 'sound' | 'tier' | 'apps';

export type Field = {
  key: string;
  label: string;
  kind: FieldKind;
  section: 'Capture' | 'Quality' | 'Audio' | 'Clips' | 'Shortcuts' | 'Sounds' | 'App' | 'Updates';
  /** The one line the page shows under the label. */
  help: string;
  /** The rest, behind « Détails », for what most people never need. */
  more?: string;
  /** Hidden behind « Avancé » at the end of its section. */
  advanced?: true;
  /** The setting this row only matters under, and what to say when it is not. */
  onlyWhen?: { key: string; value: string; reason: string };
  min?: number;
  max?: number;
  options?: { value: string; label: string }[];
  /** The select's values are numbers on the wire (shown as strings). */
  numeric?: boolean;
  /**
   * A `tier` field's presets, in dropdown order. Lives on the descriptor
   * rather than in a table `Field.tsx` looks up by `field.key`, for the
   * same reason `liveTest` does: this page has already shipped one Critical
   * from a control keyed on a hard-coded config-key literal, and a second
   * tiered setting would repeat it.
   */
  tiers?: { value: number; label: string }[];
  /** An `apps` field's wording: what it says when the list is empty, and its add button. */
  picker?: { empty: string; add: string };
  /** Filled at runtime from monitors.list / encoders.list. */
  dynamic?: 'monitors';
  /**
   * Whether a press of this hotkey reaches the daemon as `hotkey_pressed`,
   * which is what `Field.tsx`'s "Test" button and its hint are answering.
   * Only `clip_hotkey`'s `WM_HOTKEY` arm broadcasts that event -- the
   * screenshot arm deliberately stays silent (`window.rs`, the arm for
   * `SHOT_HOTKEY_ID`) so a screenshot press can never make the clip row's
   * "press it now" check look answered.
   *
   * A flag on the descriptor rather than a `field.key === 'clip_hotkey'`
   * string check in the component: this branch already shipped one Critical
   * from a hard-coded `clip_hotkey` overwriting the wrong setting, and tying
   * a UI affordance to a literal key name is the same shape of mistake
   * waiting for a third hotkey row. `true` is the only value this ever takes,
   * so a missing flag and an absent one both mean "not live-testable" with
   * nothing to get out of sync.
   */
  liveTest?: true;
  /**
   * Which `status` field says whether this hotkey is actually listening.
   *
   * On the descriptor for the same reason `liveTest` is, and not derived by
   * building `${key}_bound` at the call site: a name assembled from a string
   * is a name TypeScript cannot check, and the day the daemon publishes a key
   * that does not follow the pattern the warning would just quietly stop
   * appearing. A row that declares nothing here shows no warning, which is
   * the right default for a row nobody has wired up.
   */
  boundKey?: 'clip_hotkey_bound' | 'screenshot_hotkey_bound';
};

/**
 * Bounds mirrored from `state.rs`'s NUMERIC_BOUNDS.
 *
 * Duplicated deliberately, and it is not a DRY violation to fix: the daemon
 * must keep validating for third-party clients that never see this file, and
 * this copy exists only so a typo is caught before a round trip. The daemon
 * remains the authority -- if the two ever disagree, its answer is the one the
 * user sees.
 */
const BOUNDS: Record<string, [number, number]> = {
  fps: [30, 120],
  bitrate_kbps: [0, 200000],
  max_bitrate_kbps: [0, 200000],
  replay_seconds: [1, 600],
  monitor_index: [0, 63],
  stats_seconds: [0, 86400],
  max_library_gb: [0, 10000],
  system_volume: [0, 200],
  mic_volume: [0, 200],
  auto_clip_min_kills: [1, 5],
};

/**
 * The value `tierFor` reports when a saved number matches no preset, and the
 * option the dropdown shows for it. Not a number, so it can never collide
 * with a real bitrate.
 */
export const CUSTOM_TIER = 'custom';

/**
 * Target-bitrate presets, in kbit/s.
 *
 * Not stored anywhere: `bitrate_kbps` remains the plain number the daemon and
 * `config.toml` have always held, and the tier is derived back out of it by
 * `tierFor`. A saved tier would be a second source of truth for one value,
 * and the two would disagree the first time somebody edited config.toml by
 * hand.
 *
 * The steps are roughly 1.5x apart, and 8000 -- the shipped default, and the
 * bitrate every published Clippy measurement was taken at -- is deliberately a
 * preset rather than a boundary, so an existing install shows `Medium` with
 * nothing migrated.
 */
export const BITRATE_TIERS: { value: number; label: string }[] = [
  { value: 0, label: 'Automatique – selon l’écran et les fps' },
  { value: 5000, label: 'Faible – 5 000 kbps' },
  { value: 8000, label: 'Moyen – 8 000 kbps' },
  { value: 14000, label: 'Élevé – 14 000 kbps' },
  { value: 20000, label: 'Très élevé – 20 000 kbps' },
];

/**
 * Which dropdown entry a saved value shows as: the matching preset, or
 * `CUSTOM_TIER` for anything else -- including a number typed into an older
 * Clippy, which must round-trip back into the box rather than being rounded
 * onto the nearest preset behind the user's back.
 */
export function tierFor(field: Field, value: unknown): string {
  const n = Number(value);
  return (field.tiers ?? []).some((t) => t.value === n) ? String(n) : CUSTOM_TIER;
}

/** The dropdown's options: the presets, then Custom. */
export function tierOptions(field: Field): { value: string; label: string }[] {
  return [
    ...(field.tiers ?? []).map((t) => ({ value: String(t.value), label: t.label })),
    { value: CUSTOM_TIER, label: 'Personnalisé…' },
  ];
}

/** How many applications the daemon accepts in `audio_apps` (`config.rs`'s `MAX_AUDIO_APPS`). */
export const MAX_AUDIO_APPS = 8;

/** An executable name as people say it: `spotify.exe` is "Spotify". */
export function appLabel(exe: string): string {
  const stem = exe.replace(/\.exe$/i, '');
  return stem.charAt(0).toUpperCase() + stem.slice(1);
}

/**
 * The list with `exe` added, or the same list when there is nothing to add.
 *
 * Cleans what a person typed the way the daemon will (bare file name, lower
 * case) so the chip shown is the name that gets matched, and gives a bare word
 * its extension: "spotify" means `spotify.exe`. Returns the very same array
 * when nothing changes, which is how a caller tells.
 */
export function withApp(list: string[], exe: string): string[] {
  let name = (exe.split(/[\\/]/).pop() ?? '').trim().toLowerCase();
  if (name && !name.includes('.')) name += '.exe';
  if (!name || list.includes(name) || list.length >= MAX_AUDIO_APPS) return list;
  return [...list, name];
}

export function withoutApp(list: string[], exe: string): string[] {
  return list.filter((e) => e !== exe);
}

export const FIELDS: Field[] = [
  { key: 'capture_mode', label: 'Quoi enregistrer', kind: 'select', section: 'Capture', options: [
      { value: 'desktop', label: 'Jeux et bureau' },
      { value: 'game', label: 'Jeux uniquement' },
    ], help: 'Jeux uniquement : n’enregistre que lorsqu’un jeu est au premier plan, jamais le bureau. Jeux et bureau : tout l’écran. La liste des jeux se gère dans la vue « Jeux ».',
    more: 'Comme dans l’application NVIDIA. « Jeux uniquement » attend qu’un jeu soit au premier plan en plein écran (ou ajouté dans la vue « Jeux »), enregistre l’écran où il se trouve et s’arrête dès qu’il se ferme : le bureau, le navigateur et les lecteurs vidéo ne sont jamais capturés. S’applique immédiatement.' },
  { key: 'monitor_index', label: 'Écran', kind: 'select', section: 'Capture', dynamic: 'monitors',
    onlyWhen: { key: 'capture_mode', value: 'desktop', reason: 'Avec « Jeux uniquement », c’est l’écran du jeu qui est capturé' },
    help: 'Écran à capturer.' },
  { key: 'fps', label: 'Fréquence d’images', kind: 'number', section: 'Capture', ...span('fps'), help: 'Fréquence de capture et d’encodage.' },
  { key: 'replay_seconds', label: 'Tampon de replay', kind: 'number', section: 'Capture', ...span('replay_seconds'), help: 'Secondes conservées en mémoire (RAM). La mémoire utilisée est proportionnelle à cette durée multipliée par le débit.' },
  { key: 'gpu_priority', label: 'Priorité GPU', kind: 'select', section: 'Capture', advanced: true, options: [
      { value: 'low', label: 'Faible – aucun impact sur les fps' },
      { value: 'normal', label: 'Normale – capture plus fluide' },
    ], help: 'Faible abandonne des images de capture quand le GPU est saturé, plutôt que de ralentir le jeu.' },
  { key: 'capture_method', label: 'Méthode de capture', kind: 'select', section: 'Capture', advanced: true, options: [
      { value: 'auto', label: 'Automatique' },
      { value: 'wgc', label: 'Windows Graphics Capture' },
      { value: 'dd', label: 'Duplication du Bureau' },
    ], help: 'Si Windows dessine une bordure jaune autour de l’écran, choisir Duplication du Bureau. Elle n’enregistre pas le curseur de la souris.',
    more: 'Duplication du Bureau n’est pas soumise à cette bordure, mais ne peut pas enregistrer le curseur de la souris, ni dans les clips ni dans les captures d’écran. Automatique correspond à Windows Graphics Capture.' },

  { key: 'bitrate_kbps', label: 'Débit cible', kind: 'tier', section: 'Quality', ...span('bitrate_kbps'), tiers: BITRATE_TIERS,
    help: 'Données allouées à chaque seconde de vidéo. Plus c’est élevé, mieux les mouvements rapides sont tenus, mais clips et RAM grossissent.',
    more: 'Le tampon de replay conserve autant de secondes que réglé : un débit plus élevé occupe donc plus de RAM, et plus d’espace disque par clip. Personnalisé accepte n’importe quelle valeur.' },
  { key: 'max_bitrate_kbps', label: 'Débit maximal', kind: 'number', section: 'Quality', advanced: true, ...span('max_bitrate_kbps'), help: '0 équivaut à 1,5 fois le débit cible. Ce plafond détermine aussi l’utilisation maximale de RAM du tampon de replay.' },
  { key: 'rate_control', label: 'Contrôle du débit', kind: 'select', section: 'Quality', advanced: true, options: [
      { value: 'vbr', label: 'VBR – qualité prioritaire' },
      { value: 'cbr', label: 'CBR – taille prévisible' },
    ], help: 'Façon dont l’encodeur répartit son débit.' },

  { key: 'system_volume', label: 'Son du PC', kind: 'slider', section: 'Audio', ...span('system_volume'),
    help: 'Volume du son du PC dans le clip, sans toucher à celui de Windows. 100 laisse le son inchangé, 0 le coupe.',
    more: 'Au-delà de 100, Clippy amplifie le son, ce qui peut provoquer de la distorsion. N’affecte que l’enregistrement.' },
  { key: 'mic_volume', label: 'Microphone', kind: 'slider', section: 'Audio', ...span('mic_volume'),
    help: 'Volume de la voix dans le clip. 100 laisse le microphone inchangé, 0 le coupe.',
    more: 'Au-delà de 100, Clippy amplifie la voix : utile si le micro reste trop faible même au maximum dans Windows, mais peut provoquer de la distorsion. À 0, Windows cesse d’indiquer que Clippy utilise le microphone.' },
  { key: 'audio_apps', label: 'Son enregistré', kind: 'apps', section: 'Audio',
    help: 'Par défaut, tout le son du PC. Ajouter des applications pour n’enregistrer que leur son. Avec « Jeux uniquement » : le son du jeu seul (profils : vue « Jeux »).',
    more: 'Une application apparaît dans la liste dès qu’elle a produit du son. « Autre application » accepte aussi le nom de son exécutable.' },

  { key: 'clip_dir', label: 'Dossier des clips', kind: 'folder', section: 'Clips', help: 'Emplacement d’enregistrement des clips. Vide : Videos\\Clippy.' },
  { key: 'group_by_game', label: 'Ranger les clips par jeu', kind: 'bool', section: 'Clips', help: 'Un sous-dossier par jeu dans le dossier des clips (Bureau pour le reste), comme l’application NVIDIA. Pris en compte au prochain démarrage du replay.' },
  { key: 'auto_clip', label: 'Clips automatiques', kind: 'bool', section: 'Clips', help: 'Un clip tout seul après vos kills, multi-kills et aces dans Counter-Strike 2, League of Legends et Valorant. Le replay doit être actif. CS2 : redémarrez le jeu après l’activation.',
    more: 'Clippy utilise les interfaces officielles des jeux, sans lire leur mémoire : l’intégration d’état de jeu de Valve (fichier gamestate_integration_clippy.cfg ajouté au dossier cfg de CS2, lu au lancement du jeu) l’API Live Client Data de Riot pour League of Legends et, pour Valorant, le kill feed lu dans l’image déjà enregistrée (votre portrait encadré de jaune).' },
  { key: 'auto_clip_min_kills', label: 'Kills pour un clip automatique', kind: 'select', section: 'Clips', options: [
      { value: '1', label: 'Chaque kill' },
      { value: '2', label: '2 kills rapprochés ou plus' },
      { value: '3', label: '3 kills rapprochés ou plus' },
      { value: '4', label: '4 kills rapprochés ou plus' },
      { value: '5', label: '5 kills (ace)' },
    ], numeric: true, help: 'Les kills à quelques secondes d’intervalle forment un seul clip. Un ace ou un penta kill est toujours enregistré.' },
  { key: 'max_library_gb', label: 'Limite de la bibliothèque', kind: 'number', section: 'Clips', ...span('max_library_gb'), help: 'En Go. Au-delà de la limite, les clips non favoris les plus anciens sont supprimés. 0 désactive la limite.' },

  { key: 'clip_hotkey', label: 'Raccourci du clip', kind: 'hotkey', section: 'Shortcuts', liveTest: true, boundKey: 'clip_hotkey_bound',
    help: 'Appuyer sur la combinaison pour la tester. Si rien ne se passe en jeu, essayer Détection des raccourcis plus bas.',
    more: 'Si le test ne fait rien, ou s’il fonctionne ici mais que le jeu ignore la touche, essayer Détection des raccourcis plus bas : les surcouches et certains jeux interceptent la touche avant que Clippy ne la voie.' },
  { key: 'screenshot_hotkey', label: 'Raccourci de capture d’écran', kind: 'hotkey', section: 'Shortcuts', boundKey: 'screenshot_hotkey_bound', help: 'Enregistre une image de l’écran et la copie dans le presse-papiers. Ne fonctionne que lorsque le replay est actif.',
    more: 'L’image provient de l’enregistrement déjà en cours.' },
  { key: 'hotkey_mode', label: 'Détection des raccourcis', kind: 'select', section: 'Shortcuts', options: [
      { value: 'standard', label: 'Standard' },
      { value: 'low_level', label: 'Bas niveau – détection garantie' },
    ], help: 'À essayer si un raccourci ne fait rien dans un jeu. « Bas niveau » lit le clavier directement.',
    more: 'S’applique aux deux raccourcis. Standard demande à Windows de réserver la combinaison : la demande est refusée d’emblée si un autre programme l’utilise déjà, et même lorsqu’elle aboutit, certains jeux et surcouches interceptent la touche avant que Windows ne la transmette (constaté dans Euro Truck Simulator 2, où Clippy détenait la touche sans jamais voir d’appui). Bas niveau surveille directement le clavier : la touche est vue dans les deux cas, puis retransmise afin que les autres programmes qui l’utilisent continuent de fonctionner. Certains logiciels anti-triche se méfient des programmes qui surveillent le clavier ; OBS, Discord et Steam le font aussi, mais Clippy ne peut pas garantir comment un anti-triche donné l’interprète. Aucun des deux modes n’aide si le jeu s’exécute en tant qu’administrateur et que Clippy ne le fait pas.' },

  { key: 'screenshot_sound', label: 'Son de capture d’écran', kind: 'bool', section: 'Sounds', help: 'Un bref son à l’enregistrement d’une capture d’écran, différent de celui du clip pour les distinguer sans regarder.' },
  { key: 'clip_banner', label: 'Bandeau à l’écran', kind: 'bool', section: 'Sounds', help: 'Affiche « Clip enregistré » en haut à droite, par-dessus le jeu, à chaque clip ou capture. N’apparaît jamais dans les enregistrements.',
    more: 'Visible par-dessus les jeux en fenêtré ou en plein écran fenêtré. Un jeu en plein écran exclusif le masque.' },
  { key: 'clip_sound', label: 'Son du clip', kind: 'bool', section: 'Sounds', help: 'Joue un son à l’enregistrement d’un clip, même lorsque la fenêtre de Clippy est fermée.' },
  { key: 'clip_sound_path', label: 'Fichier son', kind: 'sound', section: 'Sounds', help: 'Un son personnel, ou celui intégré à Clippy. mp3, wav, m4a et tout autre format que Windows sait lire. Seules les 10 premières secondes sont utilisées.' },

  { key: 'autostart', label: 'Lancer au démarrage de Windows', kind: 'bool', section: 'App', help: 'Désactivé par défaut. Écrit l’entrée Run du registre, qui fait foi.' },
  { key: 'auto_arm', label: 'Activer le replay au lancement de Clippy', kind: 'bool', section: 'App',
    help: 'Remplit le tampon de replay dès le lancement de Clippy. À associer avec « Lancer au démarrage de Windows ».',
    more: 'Prend effet au prochain lancement de Clippy et mobilise l’encodeur ainsi que la mémoire du tampon tant que le replay reste actif.' },
  { key: 'discord_presence', label: 'Présence Discord', kind: 'bool', section: 'App',
    help: 'Affiche « Clipping with Clippy » sur le profil Discord, avec un bouton pour que les amis l’obtiennent.',
    more: 'Ne communique qu’avec l’application Discord de ce PC et n’envoie aucune information personnelle.' },
  { key: 'diagnostics', label: 'Journal de diagnostic', kind: 'bool', section: 'App', help: 'Enregistre en détail ce qui se passe dans Clippy et les performances du PC, pour analyser un problème. Reste sur ce PC.',
    more: 'Écrit dans %APPDATA%\clippy\logs : journal détaillé de l’enregistreur, actions dans la fenêtre (jamais le texte saisi), et toutes les 2 secondes processeur, mémoire, carte graphique et statistiques de capture. Les fichiers de plus de 14 jours sont supprimés.' },
  { key: 'stats_seconds', label: 'Intervalle des statistiques', kind: 'number', section: 'App', advanced: true, ...span('stats_seconds'), help: 'Secondes entre deux rapports de performances. 0 les désactive.' },

  { key: 'check_for_updates', label: 'Rechercher les mises à jour', kind: 'bool', section: 'Updates', help: 'Interroge github.com une fois par lancement pour savoir si une version plus récente existe. Rien n’est installé sans validation.',
    more: 'N’envoie aucune information personnelle.' },
];

/** A field's help in full: the line the page shows and the details behind « Détails ». */
export function fullHelp(field: Field): string {
  return field.more ? `${field.help} ${field.more}` : field.help;
}

/**
 * Why this row currently has no effect, or null when it does -- or when the
 * setting it depends on has not loaded yet, which must not dim the whole page.
 */
export function inactiveReason(field: Field, config: Record<string, unknown>): string | null {
  const gate = field.onlyWhen;
  if (!gate) return null;
  const current = config[gate.key];
  return current === undefined || current === gate.value ? null : gate.reason;
}

/**
 * The sections `Settings.tsx` renders, in display order, and the single
 * source of truth for it: the page iterates this constant instead of its own
 * literal, so a `FIELDS` entry naming a section absent here is invisible
 * rather than silently unrendered. `Updates` sits last because it is an
 * app-level concern, below the capture and clip settings above it.
 */
export const SECTIONS: Field['section'][] = ['Capture', 'Quality', 'Audio', 'Clips', 'Shortcuts', 'Sounds', 'App', 'Updates'];

/**
 * What each section's heading says. The `section` values themselves are
 * identifiers (compared, iterated and used as keys), so the words a person
 * reads live here instead of replacing them. « Capture » is kept for screenshots
 * only, so the recording section is « Enregistrement ».
 */
export const SECTION_LABELS: Record<Field['section'], string> = {
  Capture: 'Enregistrement',
  Quality: 'Qualité',
  Audio: 'Audio',
  Clips: 'Clips',
  Shortcuts: 'Raccourcis',
  Sounds: 'Sons',
  App: 'Application',
  Updates: 'Mises à jour',
};

function span(key: string): { min: number; max: number } {
  const [min, max] = BOUNDS[key];
  return { min, max };
}

/**
 * The read-only extras `config.get` adds on top of the real config keys
 * (`state.rs`'s `config_json`): `clip_dir_resolved`, the absolute directory an
 * empty `clip_dir` actually resolves to, and `config_file_exists`, spec §7.4's
 * "no file means first run" flag. Neither is a config key, neither is
 * settable, and neither belongs in config.toml -- so `unknownKeys` must not
 * flag them, or every user sees a false "this daemon has settings this app
 * does not render yet" banner on every visit (they did, once: this exact pair
 * is why this comment names both by name instead of trusting a future reader
 * to rediscover it from `state.rs`).
 *
 * Exported, not inlined into `unknownKeys`, so `settings.test.ts` can build
 * its fixture from this list instead of a hand-typed copy of it -- the two
 * cannot drift apart from each other, even though neither can see a key the
 * daemon grows that nobody adds here too.
 */
// `game_apps` is the legacy list, superseded by the Jeux view; a daemon may still report it.
export const READ_ONLY_EXTRAS = ['clip_dir_resolved', 'config_file_exists', 'game_apps'];

/** Keys `config.get` returned that this page has no field for. */
export function unknownKeys(config: Record<string, unknown>): string[] {
  const rendered = new Set([...FIELDS.map((f) => f.key), ...READ_ONLY_EXTRAS]);
  return Object.keys(config).filter((key) => !rendered.has(key));
}

/** The daemon's own bound, checked early. `null` means acceptable. */
export function validate(key: string, value: unknown): string | null {
  const bound = BOUNDS[key];
  if (!bound || typeof value !== 'number') return null;
  const [min, max] = bound;
  const fr = (n: number) => n.toLocaleString('fr-FR');
  const name = FIELDS.find((f) => f.key === key)?.label ?? key;
  return value < min || value > max ? `${name} accepte les valeurs de ${fr(min)} à ${fr(max)}` : null;
}

/**
 * What a `hotkey` field's Save button commits: always the field's own key,
 * paired with the combination just captured.
 *
 * Pulled out of `Field.tsx` and into this plain module so it can be
 * pinned by a test -- the web suite has no DOM (see the rest of
 * `lib/*.test.ts`), so a `<button onclick>` inside a component can
 * never be driven directly from a test, but a function it calls can be. This
 * is exactly the seam that was missing when the Screenshot row's Save button
 * shipped calling `set('clip_hotkey', capture)` -- a string typed once for
 * the only hotkey there was, and never revisited when a second one arrived.
 */
export function hotkeySaveTarget(field: Field, capture: string): { key: string; value: string } {
  return { key: field.key, value: capture };
}

/**
 * Why this hotkey row is not working, or null when it is -- or when nobody
 * knows yet.
 *
 * `bound` is the daemon's tri-state (`clip_hotkey_bound` /
 * `screenshot_hotkey_bound` on `status`): true bound, false refused, and
 * null/undefined for "the pump has not tried yet", which every client sees
 * for a moment at startup and forever in a build with no window. Only an
 * explicit `false` is a problem -- warning on the unknown would put "your
 * hotkey is taken" on screen at every launch, which is the version of this
 * warning nobody reads by the third time.
 *
 * The advice depends on which mechanism failed, because the two fail for
 * opposite reasons. Standard mode is refused when another program already
 * owns the combination, and the answer is a different key or the low-level
 * mode that does not need to own anything. Low-level mode is refused when
 * Windows would not install the hook at all, and the answer is the other way
 * round.
 *
 * Pure and exported for the same reason `hotkeySaveTarget` is: the web suite
 * runs on node with no DOM, so the sentence a user reads can only be pinned
 * by a test if it is produced by a function rather than by markup.
 */
export function hotkeyProblem(
  field: Field,
  bound: boolean | null | undefined,
  config: Record<string, unknown>,
): string | null {
  if (field.kind !== 'hotkey' || bound !== false) return null;
  const typed = String(config[field.key] ?? '').toUpperCase();
  return String(config['hotkey_mode'] ?? 'standard') === 'low_level'
    ? `Windows n’a pas permis à Clippy de surveiller le clavier pour ${typed ? `la combinaison ${typed}` : 'cette combinaison'}. Remettre Détection des raccourcis sur Standard, ou choisir une autre combinaison.`
    : `${typed ? `La combinaison ${typed}` : 'Cette combinaison'} est déjà utilisée par un autre programme : Clippy ne la reçoit donc jamais. Choisir une autre combinaison, ou régler Détection des raccourcis sur Bas niveau pour la capter malgré tout.`;
}

/**
 * `hotkeyProblem` for the clip hotkey, for a caller that renders that one row
 * without the settings list around it.
 *
 * Looks the descriptor up rather than taking one, so `FirstRun.tsx` gets
 * the warning without also getting a second opinion about which field the
 * clip hotkey is -- the mistake `liveTest` and `boundKey` exist to prevent.
 */
export function clipHotkeyProblem(
  bound: boolean | null | undefined,
  config: Record<string, unknown>,
): string | null {
  const field = FIELDS.find((f) => f.key === 'clip_hotkey');
  return field ? hotkeyProblem(field, bound, config) : null;
}

/**
 * What a `hotkey` field's keycap recorder seeds `capture` with when the user
 * clicks in to arm it -- the field's own current value, not another field's.
 * Companion to `hotkeySaveTarget` above, extracted for the same reason: this
 * is the read-side half of the same bug (`Field.tsx` used to seed every
 * hotkey row from `config['clip_hotkey']`).
 */
export function hotkeySeed(field: Field, config: Record<string, unknown>): string {
  return String(config[field.key] ?? '');
}

/**
 * The combination a key event names, or null for an event that names none.
 *
 * One definition for both recorders -- the settings row and the setup step.
 * Two copies would be two chances to disagree about what the Windows key is
 * called or which keys count as modifiers, and the daemon parses exactly one
 * grammar (`control::Hotkey::parse`).
 *
 * Null while only a modifier is held, which is what lets a recorder keep
 * waiting rather than save `alt` as a hotkey the instant Alt goes down.
 *
 * Pure and exported for the same reason `hotkeySaveTarget` is: the web suite
 * runs on node with no DOM, so this can only be pinned by a test if it is a
 * function rather than a handler body inside a component.
 */
export function comboFromEvent(e: KeyboardEvent): string | null {
  const key = e.key.toLowerCase();
  if (['control', 'alt', 'shift', 'meta'].includes(key)) return null;
  const parts: string[] = [];
  if (e.ctrlKey) parts.push('ctrl');
  if (e.altKey) parts.push('alt');
  if (e.shiftKey) parts.push('shift');
  if (e.metaKey) parts.push('win');
  parts.push(key);
  return parts.join('+');
}
