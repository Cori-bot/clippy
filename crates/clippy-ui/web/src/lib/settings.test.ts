import { describe, expect, it } from 'vitest';
import { BITRATE_TIERS, CUSTOM_TIER, FIELDS, fullHelp, inactiveReason, MAX_AUDIO_APPS, appLabel, withApp, withoutApp, READ_ONLY_EXTRAS, SECTIONS, SECTION_LABELS, clipHotkeyProblem, comboFromEvent, hotkeyProblem, hotkeySaveTarget, hotkeySeed, tierFor, tierOptions, unknownKeys, validate } from './settings';

describe('FIELDS', () => {
  it('covers every config key the daemon has today', () => {
    const shipped = [
      'fps', 'bitrate_kbps', 'max_bitrate_kbps', 'rate_control', 'replay_seconds',
      'monitor_index', 'clip_hotkey', 'gpu_priority', 'capture_method', 'stats_seconds', 'clip_dir',
      'max_library_gb', 'autostart', 'system_volume', 'mic_volume', 'check_for_updates',
      'clip_sound', 'clip_sound_path', 'discord_presence',
      'screenshot_hotkey', 'screenshot_sound',
      'hotkey_mode', 'audio_apps', 'capture_mode',
    ];
    const covered = FIELDS.map((f) => f.key);
    for (const key of shipped) expect(covered).toContain(key);
  });

  /**
   * Desktop Duplication cannot capture the mouse cursor, and a user who picks
   * it and only then finds their clips have no pointer files a bug, and is
   * right to. So it must be disclosed before they choose.
   *
   * It used to be disclosed twice -- in the option label and in the help --
   * and the label half was dropped deliberately on 2026-09-07: the labels had
   * grown long enough to run off the side of the window. (`Select.tsx`'s
   * popup no longer overflows either way, so if the label warning is ever
   * wanted back it can come back safely.) The help is now the only place that
   * says it, which makes this assertion the whole of the disclosure rather
   * than one of two, and is why it is worth keeping even though it looks
   * like a spot check on wording.
   *
   * When the cursor lands on this backend, this comes out in the same commit
   * -- and this test is what fails until it does.
   */
  it('discloses what Desktop Duplication costs', () => {
    const field = FIELDS.find((f) => f.key === 'capture_method');
    expect(field, 'capture_method must be a rendered field').toBeDefined();
    const dd = field!.options?.find((o) => o.value === 'dd');
    expect(dd, 'Desktop Duplication must be offered').toBeDefined();
    expect(fullHelp(field!).toLowerCase()).toContain('curseur');
    // The symptom, not the API: a user picks this because of what they can see.
    expect(fullHelp(field!).toLowerCase()).toContain('bordure jaune');
    // The frame-rate warning is deliberately GONE, and this asserts its
    // absence. It was true: the backend acquired a frame on every present and
    // discarded the surplus, which starved the encoder down to 20 fps against
    // WGC's 59. That was a bug, not a property of duplication, and it is fixed
    // -- 55.4 and 55.8 fps against 58.9, and confirmed on the reporting user's
    // own machine. Copy that still warned about it would send people back to
    // the yellow border for nothing.
    expect(fullHelp(field!).toLowerCase()).not.toContain('fréquence d’images');
    expect(fullHelp(field!).toLowerCase()).not.toContain('moins d’images');
  });

  /**
   * The low-level hotkey mode is a trade, not an upgrade, and the copy has to
   * read that way. It sees a combination another program has taken; it does
   * that by watching every keystroke on the machine, which is a shape some
   * anti-cheat software distrusts. That risk lands on the user, and a user who
   * gets banned because a dropdown implied this was the better setting has
   * been failed by the wording, not by the code.
   *
   * So: the anti-cheat caveat must be present, and the words that would sell
   * it as an improvement must not be.
   */
  it('offers the low-level hotkey mode as a trade rather than an upgrade', () => {
    const field = FIELDS.find((f) => f.key === 'hotkey_mode');
    expect(field, 'hotkey_mode must be a rendered field').toBeDefined();
    expect(field!.options?.map((o) => o.value)).toEqual(['standard', 'low_level']);

    const help = fullHelp(field!).toLowerCase();
    expect(help, 'the anti-cheat caveat is the whole reason this is opt-in').toContain('anti-triche');
    // The symptom that sends someone here, in their words, not the API's.
    expect(help).toContain('ne fait rien');
    // Elevation is the failure this mode does NOT fix, and saying so is what
    // stops it being the next thing a stuck user blames.
    expect(help).toContain('administrateur');
    for (const sell of ['meilleur', 'recommand', 'amélior']) {
      expect(help, `"${sell}" would read as an endorsement`).not.toContain(sell);
    }
  });

  /**
   * The two ways a hotkey dies look identical to the user and are completely
   * different underneath, and the row has to name both or it sends half its
   * readers the wrong way.
   *
   * Refused: another program already owns the combination, `RegisterHotKey`
   * fails, and the row's own red warning says so.
   *
   * Intercepted: the registration succeeded, Clippy really does hold the key,
   * and something in the game takes the keystroke before Windows' hotkey
   * table sees it. Confirmed in Euro Truck Simulator 2 -- no warning, the
   * Test button lights up here, and the key still does nothing in the game.
   * The first version of this copy said "if the test never lights up", which
   * is advice the intercepted user reads and correctly concludes does not
   * apply to them.
   */
  it('sends both kinds of dead hotkey to the same setting', () => {
    const help = fullHelp(FIELDS.find((f) => f.key === 'clip_hotkey')!).toLowerCase();
    expect(help, 'the refused case').toContain('test ne fait rien');
    expect(help, 'the intercepted case: it works here and not in the game').toContain('fonctionne ici');
    expect(help, 'and where to go about either').toContain('détection des raccourcis');
  });

  it('renders both levels as sliders in the Audio section', () => {
    for (const key of ['system_volume', 'mic_volume']) {
      const field = FIELDS.find((f) => f.key === key);
      expect(field, `${key} has no field`).toBeDefined();
      expect(field?.kind).toBe('slider');
      expect(field?.section).toBe('Audio');
    }
  });

  it('bounds both levels at 0 to 200, mirroring the daemon', () => {
    // 100 is unity; 200 is the ceiling the mixer's MAX_VOLUME_PERCENT sets.
    // A page that let someone ask for 201 would produce a round trip that
    // fails for a reason the field cannot explain.
    expect(validate('mic_volume', 201)).toMatch(/0 à 200/);
    expect(validate('system_volume', 201)).toMatch(/0 à 200/);
    expect(validate('mic_volume', 0)).toBeNull();
    expect(validate('system_volume', 100)).toBeNull();
  });

  it('accepts the boost range both sliders can now reach', () => {
    // The half of the travel that did not exist before. If BOUNDS fell back
    // to 100 the sliders would still render to 200 and every value above
    // unity would be refused on save.
    expect(validate('mic_volume', 150)).toBeNull();
    expect(validate('system_volume', 200)).toBeNull();
  });
});

describe('bitrate tiers', () => {
  const field = FIELDS.find((f) => f.key === 'bitrate_kbps')!;

  it('renders the bitrate as a tier dropdown carrying its own presets', () => {
    // `tiers` on the descriptor, not a table keyed on 'bitrate_kbps' inside
    // the component -- see the `tiers` doc comment for why this page does not
    // key controls on config-key literals.
    expect(field.kind).toBe('tier');
    expect(field.tiers).toBe(BITRATE_TIERS);
  });

  it('shows every preset as itself', () => {
    for (const tier of BITRATE_TIERS) {
      expect(tierFor(field, tier.value), tier.label).toBe(String(tier.value));
    }
  });

  it('shows 0 -- the shipped default, derived from screen size and fps -- as Automatic', () => {
    expect(tierFor(field, 0)).toBe('0');
  });

  it('keeps 8000 a preset, so installs that saved it need no migration', () => {
    // The tier is derived from the number rather than stored, so an existing
    // config.toml is already correct -- but only while 8000 is on the list.
    // Drop it and every default install silently becomes `Custom`.
    expect(tierFor(field, 8000)).toBe('8000');
  });

  it('falls back to Custom for a number no preset matches', () => {
    // A value hand-typed into an older Clippy. It must survive as itself; a
    // dropdown that rounded it onto the nearest preset would change what the
    // user records without telling them.
    expect(tierFor(field, 9500)).toBe(CUSTOM_TIER);
    expect(tierFor(field, undefined)).toBe(CUSTOM_TIER);
  });

  it('lists the presets in order with Custom last', () => {
    expect(tierOptions(field).map((o) => o.value)).toEqual([
      '0', '5000', '8000', '14000', '20000', CUSTOM_TIER,
    ]);
  });

  it('keeps every preset inside the bounds the daemon enforces', () => {
    // A preset outside them would be a dropdown entry whose only outcome is a
    // refusal the row cannot explain.
    for (const tier of BITRATE_TIERS) {
      expect(validate('bitrate_kbps', tier.value), tier.label).toBeNull();
    }
  });

  it('rises monotonically, so the labels mean what they say', () => {
    const values = BITRATE_TIERS.map((t) => t.value);
    expect([...values].sort((a, b) => a - b)).toEqual(values);
  });
});

describe('unknownKeys', () => {
  it('ignores every read-only extra config.get adds', () => {
    // Built from `READ_ONLY_EXTRAS` -- the same list `unknownKeys` allowlists
    // -- rather than a hand-typed copy of it, so this fixture cannot drift
    // out from under the production code the way it once did: this test used
    // to be named "the two read-only extras" while carrying only one, and a
    // third (`config_file_exists`) shipped without it, showing every user a
    // false "this daemon has settings this app does not render yet" banner.
    const config: Record<string, unknown> = { fps: 60, autostart: false };
    for (const key of READ_ONLY_EXTRAS) config[key] = 'placeholder';
    expect(unknownKeys(config)).toEqual([]);
  });

  it('reports a key the daemon grew that this page does not render', () => {
    // The daemon is the source of truth for what is configurable. A settings
    // page that silently drops a new key is worse than one that renders it
    // plainly, because the user cannot tell it is missing.
    expect(unknownKeys({ fps: 60, mystery_key: 3 })).toEqual(['mystery_key']);
  });
});

describe('validate', () => {
  it('mirrors the daemon bounds so the error arrives before the round trip', () => {
    expect(validate('fps', 0)).toMatch(/30 à 120/);
    expect(validate('fps', 60)).toBeNull();
    expect(validate('replay_seconds', 601)).toMatch(/1 à 600/);
    expect(validate('max_library_gb', 0)).toBeNull();
  });

  it('says nothing about keys it has no bound for', () => {
    expect(validate('clip_hotkey', 'alt+f10')).toBeNull();
  });
});

describe('settings fields', () => {
  it('renders check_for_updates, so nobody sees the unknown-settings banner', () => {
    // config.get returns every Config key. A key with no FIELDS entry lands in
    // unknownKeys, and Settings.tsx tells the user their app is out of date
    // with their daemon -- which would be false and unfixable.
    const fromDaemon = { check_for_updates: true, ...Object.fromEntries(READ_ONLY_EXTRAS.map((k) => [k, ''])) };
    expect(unknownKeys(fromDaemon)).toEqual([]);
    expect(FIELDS.find((f) => f.key === 'check_for_updates')?.kind).toBe('bool');
  });

  it('has every FIELDS section rendered by SECTIONS', () => {
    // Settings.tsx iterates SECTIONS, not FIELDS, to decide what headings
    // to draw. A field whose `section` is not in SECTIONS is valid TypeScript
    // (tsc never checks the array literal against the union) but
    // never reaches the screen -- exactly what happened to `check_for_updates`
    // and `'Updates'` before SECTIONS existed.
    const usedSections = new Set(FIELDS.map((f) => f.section));
    for (const section of usedSections) expect(SECTIONS).toContain(section);
  });

  it('has a heading for every section it renders, Audio staying Audio', () => {
    // The `section` values are identifiers; the page reads SECTION_LABELS for
    // the words. A section without a label would render an empty heading.
    for (const section of SECTIONS) expect(SECTION_LABELS[section], section).toBeTruthy();
    expect(SECTION_LABELS.Audio).toBe('Audio');
    expect(SECTION_LABELS.Updates).toBe('Mises à jour');
  });
});

describe('the clip sound fields', () => {
  it('puts both rows in a section the page already renders', () => {
    // A field in a section `SECTIONS` does not list renders nowhere at all --
    // the settings page iterates `SECTIONS`, not `FIELDS`.
    for (const key of ['clip_sound', 'clip_sound_path']) {
      const field = FIELDS.find((f) => f.key === key);
      expect(field, `${key} must be a settings field`).toBeDefined();
      expect(SECTIONS).toContain(field!.section);
    }
  });

  it('renders the sound file row with the sound control', () => {
    expect(FIELDS.find((f) => f.key === 'clip_sound_path')!.kind).toBe('sound');
    expect(FIELDS.find((f) => f.key === 'clip_sound')!.kind).toBe('bool');
  });

  it('says which formats work, because PlaySound is not the whole story', () => {
    const help = FIELDS.find((f) => f.key === 'clip_sound_path')!.help;
    expect(help).toMatch(/mp3/i);
    expect(help).toMatch(/10 premières secondes/i);
  });
});

describe('the screenshot fields', () => {
  it('renders the hotkey with the keycap control, like the clip hotkey', () => {
    // A `hotkey` field is the keycap recorder. A `text` one would make the
    // user type "alt+f8" by hand and get the grammar wrong.
    expect(FIELDS.find((f) => f.key === 'screenshot_hotkey')!.kind).toBe('hotkey');
    expect(FIELDS.find((f) => f.key === 'screenshot_sound')!.kind).toBe('bool');
  });

  it('puts both rows in a section the page actually renders', () => {
    // Settings.tsx iterates SECTIONS, not FIELDS: a field whose section is
    // absent from SECTIONS is valid TypeScript that never reaches the screen.
    for (const key of ['screenshot_hotkey', 'screenshot_sound']) {
      expect(SECTIONS).toContain(FIELDS.find((f) => f.key === key)!.section);
    }
  });

  it('says the screenshot needs Clippy armed', () => {
    // The likeliest support question, and the help text is the only place it
    // can be answered before it is asked.
    expect(FIELDS.find((f) => f.key === 'screenshot_hotkey')!.help).toMatch(/replay est actif/i);
  });

  it('does not offer a live test, because the screenshot hotkey arm never broadcasts hotkey_pressed', () => {
    // `window.rs`'s arm for SHOT_HOTKEY_ID deliberately stays silent so a
    // screenshot press can never make the clip row's "press it now" check
    // look answered -- `Field.tsx` reads this flag to decide whether to
    // show the Test button at all.
    expect(FIELDS.find((f) => f.key === 'screenshot_hotkey')!.liveTest).toBeUndefined();
  });
});

describe('liveTest', () => {
  it('is set only for the one hotkey row the daemon actually echoes back', () => {
    // A regression guard for the pattern this replaced: `Field.tsx` used to
    // read `field.key === 'clip_hotkey'` directly, which is exactly the kind
    // of hard-coded key literal that shipped a real Critical earlier on this
    // branch. Asserting the flag here, rather than the string, is what makes
    // a future third hotkey row a decision instead of a silent default.
    const hotkeyFields = FIELDS.filter((f) => f.kind === 'hotkey');
    expect(hotkeyFields.map((f) => f.key)).toEqual(['clip_hotkey', 'screenshot_hotkey']);
    expect(FIELDS.find((f) => f.key === 'clip_hotkey')!.liveTest).toBe(true);
    expect(hotkeyFields.filter((f) => f.liveTest === true).map((f) => f.key)).toEqual([
      'clip_hotkey',
    ]);
  });
});

describe('hotkeySaveTarget', () => {
  it('sends each hotkey row to its own key, never a shared literal', () => {
    // This is the regression the first pass of this task shipped: both the
    // clip and the screenshot row saved through `set('clip_hotkey', capture)`
    // because the key was typed once, for the only hotkey field that existed
    // at the time, and never revisited when a second one arrived. A field's
    // own key must come out the other end for every hotkey field, not just
    // the one that happened to be first.
    const clip = FIELDS.find((f) => f.key === 'clip_hotkey')!;
    const screenshot = FIELDS.find((f) => f.key === 'screenshot_hotkey')!;
    expect(hotkeySaveTarget(clip, 'alt+f10')).toEqual({ key: 'clip_hotkey', value: 'alt+f10' });
    expect(hotkeySaveTarget(screenshot, 'alt+f8')).toEqual({
      key: 'screenshot_hotkey',
      value: 'alt+f8',
    });
  });
});

describe('hotkeySeed', () => {
  it('seeds capture from each field\'s own config value, never another field\'s', () => {
    // The read-side half of the same bug: seeding every hotkey row's keycaps
    // from `config['clip_hotkey']` meant clicking into the Screenshot row
    // showed the *clip* combination, ready to be saved back over itself.
    const config = { clip_hotkey: 'alt+f10', screenshot_hotkey: 'alt+f8' };
    const clip = FIELDS.find((f) => f.key === 'clip_hotkey')!;
    const screenshot = FIELDS.find((f) => f.key === 'screenshot_hotkey')!;
    expect(hotkeySeed(clip, config)).toBe('alt+f10');
    expect(hotkeySeed(screenshot, config)).toBe('alt+f8');
  });

  it('falls back to an empty string when the config has no value yet', () => {
    expect(hotkeySeed(FIELDS.find((f) => f.key === 'clip_hotkey')!, {})).toBe('');
  });
});

describe('hotkeyProblem', () => {
  const clip = FIELDS.find((f) => f.key === 'clip_hotkey')!;
  const shot = FIELDS.find((f) => f.key === 'screenshot_hotkey')!;
  const standard = { clip_hotkey: 'alt+f10', screenshot_hotkey: 'alt+f8', hotkey_mode: 'standard' };

  /**
   * The whole point of the daemon reporting a tri-state. `null` is what every
   * client sees for the moment between the daemon answering the socket and
   * the pump binding the keys, and forever in any build with no window --
   * rendering that as "your hotkey is taken" would put a false alarm on
   * screen at every launch, and a warning that cries wolf at launch is one
   * nobody reads on the launch that matters.
   */
  it('says nothing until the daemon has actually tried', () => {
    expect(hotkeyProblem(clip, null, standard)).toBeNull();
    expect(hotkeyProblem(clip, undefined, standard)).toBeNull();
  });

  it('says nothing about a hotkey that bound', () => {
    expect(hotkeyProblem(clip, true, standard)).toBeNull();
  });

  /**
   * The sentence a stuck user reads. It has to name the combination they
   * chose -- "a hotkey failed" is not actionable -- and it has to name both
   * ways out, because picking a free key and switching mechanism are
   * genuinely different answers with different costs.
   */
  it('names the combination and both ways out when Windows refused it', () => {
    const problem = hotkeyProblem(clip, false, standard)!;
    expect(problem).toContain('ALT+F10');
    expect(problem.toLowerCase()).toContain('autre programme');
    expect(problem.toLowerCase()).toContain('choisir une autre combinaison');
    expect(problem).toContain('Bas niveau');
  });

  /**
   * The advice has to invert with the mode. In low-level mode nothing was
   * "taken" -- the hook itself would not install -- so telling this user to
   * switch to Low level is advice they have already taken, and would read as
   * the setting page not knowing what it is looking at.
   */
  it('gives the opposite advice when it is the hook that failed', () => {
    const problem = hotkeyProblem(clip, false, { ...standard, hotkey_mode: 'low_level' })!;
    expect(problem.toLowerCase()).toContain('surveiller le clavier');
    expect(problem).toContain('Standard');
    expect(problem.toLowerCase()).not.toContain('autre programme');
  });

  it('reads each row own combination, not the clip one', () => {
    expect(hotkeyProblem(shot, false, standard)).toContain('ALT+F8');
  });

  it('has nothing to say about a setting that is not a hotkey', () => {
    const method = FIELDS.find((f) => f.key === 'capture_method')!;
    expect(hotkeyProblem(method, false, standard)).toBeNull();
  });

  /**
   * The wiring this depends on: a hotkey row with no `boundKey` gets its
   * `bound` argument hard-coded to null by `Settings.tsx`, so it can never
   * warn about anything. That is the safe default, and this is what notices
   * a row that was meant to be wired and was not.
   */
  it('every hotkey row declares which status field answers for it', () => {
    const rows = FIELDS.filter((f) => f.kind === 'hotkey');
    expect(rows.length).toBeGreaterThan(0);
    for (const row of rows) {
      expect(row.boundKey, `${row.key} has no boundKey`).toBe(`${row.key}_bound`);
    }
  });
});

describe('clipHotkeyProblem', () => {
  const standard = { clip_hotkey: 'alt+f10', hotkey_mode: 'standard' };

  /**
   * `FirstRun.tsx` renders the clip hotkey row without the settings list
   * around it, so it has no `Field` to hand in. This exists so it does not
   * grow a second opinion about which descriptor the clip hotkey is -- the
   * shape of mistake `liveTest` and `boundKey` were introduced to stop.
   */
  it('answers for the clip hotkey without being handed its descriptor', () => {
    const direct = hotkeyProblem(FIELDS.find((f) => f.key === 'clip_hotkey')!, false, standard);
    expect(clipHotkeyProblem(false, standard)).toBe(direct);
    expect(clipHotkeyProblem(false, standard)).toContain('ALT+F10');
  });

  /**
   * The setup wizard shows this the moment it loads, before the user has
   * pressed anything. Warning on the tri-state's unknown would put "your
   * hotkey is taken" in front of every new user during the seconds before the
   * pump reports, which is the version of the warning nobody believes.
   */
  it('stays quiet while the hotkey works, and while nobody knows yet', () => {
    expect(clipHotkeyProblem(true, standard)).toBeNull();
    expect(clipHotkeyProblem(null, standard)).toBeNull();
    expect(clipHotkeyProblem(undefined, standard)).toBeNull();
  });
});

describe('comboFromEvent', () => {
  /**
   * Not a real `KeyboardEvent`: the web suite runs on node with no DOM. Only
   * these five fields are read, which is the reason this function was pulled
   * out of `Field.tsx`'s handler in the first place.
   */
  function press(key: string, held: Partial<KeyboardEvent> = {}): KeyboardEvent {
    return { key, ctrlKey: false, altKey: false, shiftKey: false, metaKey: false, ...held } as KeyboardEvent;
  }

  it('spells a combination the way the daemon parses one', () => {
    expect(comboFromEvent(press('F10', { altKey: true }))).toBe('alt+f10');
    expect(comboFromEvent(press('F10'))).toBe('f10');
  });

  /**
   * `win`, not `meta`: this string is handed straight to `config.set` and
   * parsed by `control::Hotkey::parse`, which knows the Windows key by the
   * name the user sees on it.
   */
  it('calls the Windows key win', () => {
    expect(comboFromEvent(press('F10', { metaKey: true }))).toBe('win+f10');
  });

  /** Fixed order, so the same physical press never saves two different strings. */
  it('orders the modifiers the same way every time', () => {
    const all = { ctrlKey: true, altKey: true, shiftKey: true, metaKey: true };
    expect(comboFromEvent(press('F10', all))).toBe('ctrl+alt+shift+win+f10');
  });

  /**
   * The reason this returns null rather than a string. Alt goes down before
   * F10 does, so a recorder that took every event would save `alt` the
   * instant the user reached for the combination they meant.
   */
  it('names nothing while only a modifier is held', () => {
    for (const key of ['Control', 'Alt', 'Shift', 'Meta']) {
      expect(comboFromEvent(press(key, { altKey: true })), key).toBeNull();
    }
  });
});

describe('audio applications', () => {
  it('names an executable the way a person would', () => {
    expect(appLabel('spotify.exe')).toBe('Spotify');
    expect(appLabel('brave.EXE')).toBe('Brave');
    expect(appLabel('no-extension')).toBe('No-extension');
  });

  it('adds a cleaned name once, and hands back the same list when nothing changes', () => {
    const base = ['spotify.exe'];
    expect(withApp(base, ' Brave ')).toEqual(['spotify.exe', 'brave.exe']);
    expect(withApp(base, 'C:\\Apps\\Discord.exe')).toEqual(['spotify.exe', 'discord.exe']);
    expect(withApp(base, 'SPOTIFY.EXE')).toBe(base);
    expect(withApp(base, '   ')).toBe(base);
  });

  it('stops at the number the daemon accepts', () => {
    const full = Array.from({ length: MAX_AUDIO_APPS }, (_, i) => `app${i}.exe`);
    expect(withApp(full, 'one-more.exe')).toBe(full);
  });

  it('removes one application and leaves the rest', () => {
    expect(withoutApp(['a.exe', 'b.exe'], 'a.exe')).toEqual(['b.exe']);
    expect(withoutApp(['a.exe'], 'a.exe')).toEqual([]);
  });
});

describe('the settings page stays scannable', () => {
  it('keeps the visible help to a line or two, and the rest behind « Détails »', () => {
    // The page was 27 rows with up to 924 characters of help each. What is shown
    // is bounded; what is not needed by most people goes in `more`.
    for (const field of FIELDS) {
      expect(field.help.length, `${field.key}: ${field.help.length} characters`).toBeLessThanOrEqual(190);
    }
  });

  it('joins the details back onto the help for anything that needs the whole text', () => {
    const mode = FIELDS.find((f) => f.key === 'hotkey_mode')!;
    expect(fullHelp(mode)).toBe(`${mode.help} ${mode.more}`);
    const plain = FIELDS.find((f) => f.key === 'fps')!;
    expect(fullHelp(plain)).toBe(plain.help);
  });

  it('dims the screen choice in game mode only, and no longer lists game_apps', () => {
    const screen = FIELDS.find((f) => f.key === 'monitor_index')!;
    expect(FIELDS.some((f) => f.key === 'game_apps')).toBe(false);
    expect(inactiveReason(screen, { capture_mode: 'game' })).not.toBeNull();
    expect(inactiveReason(screen, { capture_mode: 'desktop' })).toBeNull();
  });

  it('dims nothing before the config has loaded', () => {
    for (const field of FIELDS) expect(inactiveReason(field, {}), field.key).toBeNull();
  });

  it('names the setting by its label when a bound is exceeded', () => {
    expect(validate('fps', 500)).toMatch(/^Fréquence d’images accepte/);
  });

  it('puts the expert settings behind « Avancé »', () => {
    const advanced = FIELDS.filter((f) => f.advanced).map((f) => f.key);
    expect(advanced).toEqual(['gpu_priority', 'capture_method', 'max_bitrate_kbps', 'rate_control', 'stats_seconds']);
  });
});
