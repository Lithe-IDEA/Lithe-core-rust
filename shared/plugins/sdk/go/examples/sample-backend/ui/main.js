// The page reaches its backend only through the host bridge: functions.invoke
// without an instance calls this plugin's backend, which Lithe starts on the
// first call. Inline scripts are refused by the page's content security policy.
const status = document.getElementById('status');
const note = document.getElementById('note');
const secret = document.getElementById('secret');
let calls = 0;

async function call(fn, params) {
  const id = `call-${Date.now()}-${calls++}`;
  try {
    const { value } = await lithe.request('functions.invoke', { function: fn, params, call: id });
    return value;
  } catch (error) {
    status.textContent = error.pluginErrorCode ? `${error.message} (${error.pluginErrorCode})` : error.message;
    throw error;
  }
}

lithe.on('functions.progress', ({ message }) => { if (message) status.textContent = message; });

note.addEventListener('submit', async (event) => {
  event.preventDefault();
  const { bytes } = await call('note.save', { name: note.name.value, text: note.text.value });
  status.textContent = `Saved ${bytes} bytes.`;
});

document.getElementById('read').addEventListener('click', async () => {
  note.text.value = (await call('note.read', { name: note.name.value })).text;
  status.textContent = 'Loaded.';
});

secret.addEventListener('submit', async (event) => {
  event.preventDefault();
  await call('secret.remember', { key: 'sample:password', secret: secret.secret.value });
  secret.secret.value = '';
  status.textContent = 'Stored in the system credential store.';
});

document.getElementById('check').addEventListener('click', async () => {
  const { stored, matches } = await call('secret.check', { key: 'sample:password', secret: secret.secret.value });
  status.textContent = !stored ? 'Nothing stored.' : matches ? 'It matches.' : 'It does not match.';
});
