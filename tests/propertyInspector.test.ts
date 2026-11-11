// tests/propertyInspector.test.ts
import { describe, expect, it } from 'vitest';
import {
  evaluatePropertyCondition,
  hexToWeColor,
  resolveLocalizedText,
  weColorToHex,
} from '../src/components/PropertyInspector';
import { parseImportLink } from '../src/lib/importLinks';
import type { ProjectJsonSchema } from '../src/types/ipc';

describe('Wallpaper Engine Color Conversion (weColorToHex & hexToWeColor)', () => {
  it('converts normalized 0..1 float strings to 6-digit hex', () => {
    expect(weColorToHex('0.14902 0.45098 0.85098')).toBe('#2673d9');
    expect(weColorToHex('1 0 0')).toBe('#ff0000');
    expect(weColorToHex('0 0 0')).toBe('#000000');
  });

  it('supports legacy 0..255 integer strings and passthrough hex', () => {
    expect(weColorToHex('38 115 217')).toBe('#2673d9');
    expect(weColorToHex('#38bdf8')).toBe('#38bdf8');
    expect(weColorToHex(undefined)).toBe('#ffffff');
  });

  it('converts 6-digit hex back to normalized float strings', () => {
    expect(hexToWeColor('#2673d9')).toBe('0.14902 0.45098 0.85098');
    expect(weColorToHex(hexToWeColor('#38bdf8'))).toBe('#38bdf8');
  });
});

describe('Safe Tokenized Condition Evaluator (evaluatePropertyCondition)', () => {
  const values = {
    enable_rain: true,
    weather_mode: 2,
    rain_intensity: 0.75,
    disable_particles: false,
    preset_name: 'cyberpunk',
  };

  it('returns true for empty or undefined conditions', () => {
    expect(evaluatePropertyCondition(undefined, values)).toBe(true);
    expect(evaluatePropertyCondition('   ', values)).toBe(true);
  });

  it('evaluates boolean truthiness and negation (!)', () => {
    expect(evaluatePropertyCondition('enable_rain.value', values)).toBe(true);
    expect(evaluatePropertyCondition('!disable_particles.value', values)).toBe(true);
    expect(evaluatePropertyCondition('!enable_rain.value', values)).toBe(false);
  });

  it('evaluates compound && and || numeric and string comparisons', () => {
    expect(
      evaluatePropertyCondition(
        'weather_mode.value == 2 && rain_intensity.value > 0.5',
        values
      )
    ).toBe(true);
    expect(
      evaluatePropertyCondition(
        'weather_mode.value == 1 || preset_name.value == "cyberpunk"',
        values
      )
    ).toBe(true);
    expect(
      evaluatePropertyCondition(
        'weather_mode.value != 2 && rain_intensity.value <= 0.5',
        values
      )
    ).toBe(false);
  });
});

describe('project.json Localization Lookup (resolveLocalizedText)', () => {
  const sampleProject: ProjectJsonSchema = {
    title: 'Neon Rain',
    type: 'scene',
    file: 'scene.json',
    general: {
      localization: {
        'en-us': {
          ui_browse_properties_scheme_color: 'Scheme Color',
        },
      },
    },
  };

  it('resolves localized keys and strips raw HTML tags on fallback', () => {
    expect(
      resolveLocalizedText('ui_browse_properties_scheme_color', sampleProject)
    ).toBe('Scheme Color');
    expect(resolveLocalizedText('<b>Rain Speed</b>', sampleProject)).toBe(
      'Rain Speed'
    );
  });
});

describe('Import URL and slug parser', () => {
  it('detects Steam Workshop HTTP URLs, steam:// protocol links, and numeric IDs', () => {
    expect(
      parseImportLink('https://steamcommunity.com/sharedfiles/filedetails/?id=2841928371&searchtext=')
    ).toEqual({ kind: 'steam_workshop', workshopId: '2841928371' });
    expect(
      parseImportLink('steam://url/CommunityFilePage/2841928371')
    ).toEqual({ kind: 'steam_workshop', workshopId: '2841928371' });
    expect(parseImportLink('2841928371')).toEqual({
      kind: 'steam_workshop',
      workshopId: '2841928371',
    });
  });

  it('detects Ian Truong Photography Album URLs and slugs', () => {
    expect(
      parseImportLink('https://iantruongphotography.com/album/bald-mountain-11-1')
    ).toEqual({ kind: 'photo_album', albumSlug: 'bald-mountain-11-1' });
    expect(parseImportLink('cape-horn-11-7')).toEqual({
      kind: 'photo_album',
      albumSlug: 'cape-horn-11-7',
    });
  });
});
