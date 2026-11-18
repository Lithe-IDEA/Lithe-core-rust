// Check the plugin view declarations the way a plugin uses them.
import { createBackend, type CanvasNode, type ViewNode, type ViewPatch, type ViewResult } from '@lithe/plugin-sdk';

const backend = createBackend();

backend.view('explorer.draw', (event, context) => {
  const language: string = event.language;
  const draft = event.settings.draft;
  void draft;
  void context.operationID;
  if ('settingRegionID' in event.surface) return {};
  if (event.event === 'open') {
    const graph: CanvasNode[] = [
      { type: 'path', id: 'edge', d: 'M190 20C234 20 216 80 260 80', stroke: '$line', width: 1.5 },
      {
        type: 'group', id: 'guava', x: 260, y: 60, w: 190, h: 40, on: ['click'], label: 'guava 31.1',
        states: { hover: { fill: '$hover' }, pressed: { scale: 0.97 } },
        children: [
          { type: 'rect', x: 0, y: 0, w: 190, h: 40, radius: 8, fill: { light: '#fdf0ef', dark: '#3a2726' }, dash: [4, 3] },
          { type: 'text', x: 12, y: 26, style: 'strong', runs: [{ text: 'guava' }, { text: ' 31.1', color: '$error' }] },
        ],
      },
    ];
    const view: ViewNode[] = [
      { type: 'section', title: language === 'zh-Hans' ? '模块' : 'Modules', children: [
        { type: 'row', children: [
          { type: 'textField', id: 'filter', placeholder: 'Filter', width: 'fill' },
          { type: 'segmented', id: 'scope', value: 'all', options: [{ value: 'all', label: 'All' }, { value: 'test', label: 'Test' }] },
          { type: 'button', id: 'refresh', text: 'Refresh', icon: 'arrow.clockwise' },
        ] },
        { type: 'tree', id: 'modules', items: [{ id: 'app', label: 'app', expanded: true, children: [{ id: 'core', label: 'core' }] }] },
        { type: 'canvas', id: 'graph', label: 'Module graph', height: 360, viewBox: [0, 0, 1200, 640], zoom: [0.4, 3], on: ['viewport'], nodes: graph },
      ] },
    ];
    return { view };
  }
  if (event.event === 'change' && typeof event.value === 'boolean') {
    const patches: ViewPatch[] = [
      { op: 'set', id: 'filter', fields: { enabled: event.value } },
      { op: 'set', canvas: 'graph', id: 'guava', fields: { selected: true } },
      { op: 'insert', tree: 'modules', parent: 'app', node: { id: 'web', label: 'web' } },
      { op: 'remove', id: 'refresh' },
    ];
    return { patches };
  }
  return undefined;
});

// Region controls edit a declared setting instead of holding their own value.
const region: ViewNode[] = [
  { type: 'select', id: 'otp', setting: 'otpRelease' },
  { type: 'toggle', id: 'plt', label: 'Build PLT', setting: 'buildPlt' },
  { type: 'setting', setting: 'dialyzer' },
  { type: 'tool', tool: 'erlang' },
  { type: 'button', id: 'clean', text: 'Clean', action: 'cleanPlt' },
];
void region;

// @ts-expect-error A control shows its own value or a setting, never both.
const both: ViewNode = { type: 'toggle', id: 't', label: 'T', value: true, setting: 'flag' };
// @ts-expect-error A clickable group needs its hit area and label.
const unlabeled: CanvasNode = { type: 'group', id: 'g', on: ['click'], children: [] };
// @ts-expect-error A patch addresses a canvas or a tree, never both.
const ambiguous: ViewPatch = { op: 'remove', canvas: 'graph', tree: 'modules', id: 'x' };
// @ts-expect-error Canvas text holds text or runs, never both.
const text: CanvasNode = { type: 'text', x: 0, y: 0, text: 'a', runs: [{ text: 'b' }] };
// @ts-expect-error A result is one of view, patches, or nothing.
const mixed: ViewResult = { view: [], patches: [] };
void [both, unlabeled, ambiguous, text, mixed];
