const assert = require('node:assert/strict');
const zlib = require('node:zlib');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const repo = path.join(__dirname, '..', '..');
const signing = fs.readFileSync(path.join(repo, 'scripts', 'signing.sh'), 'utf8');
const signApp = fs.readFileSync(path.join(repo, 'scripts', 'sign-app.sh'), 'utf8');
const signBinary = fs.readFileSync(path.join(repo, 'scripts', 'sign-binary.sh'), 'utf8');
const buildDeploy = fs.readFileSync(path.join(repo, 'scripts', 'build-deploy.sh'), 'utf8');
function readJsSources(dir) {
  return fs.readdirSync(dir, { withFileTypes: true })
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((entry) => {
      const file = path.join(dir, entry.name);
      if (entry.isDirectory()) return readJsSources(file);
      if (!/\.js$/.test(entry.name)) return [];
      return [fs.readFileSync(file, 'utf8')];
    });
}

function readMainSources(dir) {
  const rootFile = path.join(dir, 'main.js');
  const moduleDir = path.join(dir, 'main');
  return [fs.readFileSync(rootFile, 'utf8')].concat(readJsSources(moduleDir));
}
const main = readMainSources(path.join(repo, 'app')).join('\n');
const cliPlist = fs.readFileSync(path.join(repo, 'crates', 'alvum-cli', 'Info.plist'), 'utf8');

function readPngRgba(file) {
  const data = fs.readFileSync(file);
  assert.equal(data.subarray(0, 8).toString('hex'), '89504e470d0a1a0a');
  let offset = 8;
  let width = 0;
  let height = 0;
  let colorType = 0;
  const idat = [];
  while (offset < data.length) {
    const length = data.readUInt32BE(offset);
    const type = data.subarray(offset + 4, offset + 8).toString('ascii');
    const chunk = data.subarray(offset + 8, offset + 8 + length);
    if (type === 'IHDR') {
      width = chunk.readUInt32BE(0);
      height = chunk.readUInt32BE(4);
      assert.equal(chunk[8], 8);
      colorType = chunk[9];
      assert.equal(colorType, 6);
    } else if (type === 'IDAT') {
      idat.push(chunk);
    } else if (type === 'IEND') {
      break;
    }
    offset += 12 + length;
  }
  const inflated = zlib.inflateSync(Buffer.concat(idat));
  const stride = width * 4;
  const pixels = Buffer.alloc(height * stride);
  let inOffset = 0;
  for (let y = 0; y < height; y += 1) {
    const filter = inflated[inOffset];
    inOffset += 1;
    const row = inflated.subarray(inOffset, inOffset + stride);
    inOffset += stride;
    for (let x = 0; x < stride; x += 1) {
      const left = x >= 4 ? pixels[y * stride + x - 4] : 0;
      const up = y > 0 ? pixels[(y - 1) * stride + x] : 0;
      const upLeft = y > 0 && x >= 4 ? pixels[(y - 1) * stride + x - 4] : 0;
      if (filter === 0) pixels[y * stride + x] = row[x];
      else if (filter === 1) pixels[y * stride + x] = (row[x] + left) & 0xff;
      else if (filter === 2) pixels[y * stride + x] = (row[x] + up) & 0xff;
      else if (filter === 3) pixels[y * stride + x] = (row[x] + Math.floor((left + up) / 2)) & 0xff;
      else if (filter === 4) {
        const p = left + up - upLeft;
        const pa = Math.abs(p - left);
        const pb = Math.abs(p - up);
        const pc = Math.abs(p - upLeft);
        const predictor = pa <= pb && pa <= pc ? left : pb <= pc ? up : upLeft;
        pixels[y * stride + x] = (row[x] + predictor) & 0xff;
      } else {
        throw new Error(`unsupported PNG filter ${filter}`);
      }
    }
  }
  return pixels;
}

test('signing scripts prefer Developer ID while allowing explicit identity override', () => {
  assert.match(signing, /ALVUM_SIGN_IDENTITY/);
  assert.match(signing, /Developer ID Application:/);
  assert.match(signing, /alvum_resolve_sign_identity/);
  assert.match(signing, /alvum_codesign_args/);
  assert.match(signing, /alvum_sign_identity_available/);
});

test('bundle and binary signing share one identity resolver', () => {
  assert.match(signApp, /source "\$\(dirname "\$0"\)\/signing\.sh"/);
  assert.match(signApp, /CERT_NAME="\$\(alvum_resolve_sign_identity\)"/);
  assert.match(signApp, /alvum_codesign_args "\$CERT_NAME"/);
  assert.doesNotMatch(signApp, /CERT_NAME="alvum-dev"/);

  assert.match(signBinary, /source "\$\(dirname "\$0"\)\/signing\.sh"/);
  assert.match(signBinary, /CERT_NAME="\$\(alvum_resolve_sign_identity\)"/);
  assert.match(signBinary, /alvum_codesign_args "\$CERT_NAME"/);
  assert.doesNotMatch(signBinary, /CERT_NAME="alvum-dev"/);
});

test('deploy script signs inner bundle binary with resolved identity', () => {
  assert.match(buildDeploy, /source "\$\(dirname "\$0"\)\/signing\.sh"/);
  assert.match(buildDeploy, /CERT_NAME="\$\(alvum_resolve_sign_identity\)"/);
  assert.match(buildDeploy, /codesign "\$\{ALVUM_CODESIGN_ARGS\[@\]\}" "\$inner"/);
  assert.doesNotMatch(buildDeploy, /codesign --sign alvum-dev/);
});

test('deploy relaunch skips capture auto-start unless explicitly requested', () => {
  assert.match(buildDeploy, /skip_capture_autostart=1/);
  assert.match(buildDeploy, /--start-capture\) skip_capture_autostart=0; shift ;;/);
  assert.match(buildDeploy, /launch-intent\.json/);
  assert.match(buildDeploy, /"skip_capture_autostart":true/);
  assert.match(buildDeploy, /launch intent: skip capture auto-start once/);
  assert.match(buildDeploy, /clear_launch_intent/);
  assert.match(buildDeploy, /rm -f "\$ALVUM_RUNTIME\/launch-intent\.json"/);
});

test('packaged capture process is a helper app with icon metadata', () => {
  assert.match(main, /Alvum Capture\.app/);
  assert.match(main, /['"]Helpers['"],\s*['"]Alvum Capture\.app/);
  assert.match(buildDeploy, /helper_app="\$bundle\/Contents\/Helpers\/Alvum Capture\.app"/);
  assert.match(buildDeploy, /helper_resources="\$helper_app\/Contents\/Resources"/);
  assert.match(buildDeploy, /install_app_icon_metadata "\$bundle"/);
  assert.match(buildDeploy, /install_app_icon_metadata "\$helper_app"/);
  assert.match(buildDeploy, /install_app_icon_metadata "\$ALVUM_APP_DIR"/);
  assert.match(buildDeploy, /xcrun actool/);
  assert.match(buildDeploy, /Assets\.car/);
  assert.match(buildDeploy, /CFBundleIconName AppIcon/);
  assert.match(buildDeploy, /crates\/alvum-cli\/Info\.plist/);
  assert.match(buildDeploy, /icon\.icns/);
  assert.match(signApp, /Contents\/Helpers.*\*\.app/);
  assert.match(cliPlist, /<key>CFBundleDisplayName<\/key>\s*<string>Alvum<\/string>/);
  assert.match(cliPlist, /<key>CFBundleIconFile<\/key>\s*<string>icon\.icns<\/string>/);
  assert.match(cliPlist, /<key>CFBundleIconName<\/key>\s*<string>AppIcon<\/string>/);
});

test('active tray icon stays a template image so macOS tints it with the menu bar', () => {
  assert.ok(fs.existsSync(path.join(repo, 'app', 'assets', 'tray-icon-active.png')));
  assert.doesNotMatch(main, /tray-icon-active-light\.png/);
  assert.doesNotMatch(main, /tray-icon-active-dark\.png/);
  assert.doesNotMatch(main, /nativeTheme\.shouldUseDarkColors/);
  assert.doesNotMatch(main, /green dot/i);
  assert.match(main, /img\.setTemplateImage\(true\)/);
  assert.doesNotMatch(main, /img\.setTemplateImage\(false\)/);
});

test('active tray icon asset has no preserved green capture dot', () => {
  const pixels = readPngRgba(path.join(repo, 'app', 'assets', 'tray-icon-active.png'));
  for (let i = 0; i < pixels.length; i += 4) {
    const alpha = pixels[i + 3];
    if (alpha === 0) continue;
    const red = pixels[i];
    const green = pixels[i + 1];
    const blue = pixels[i + 2];
    assert.ok(!(green > 160 && red < 80 && blue < 140), `unexpected green pixel at rgba(${red}, ${green}, ${blue}, ${alpha})`);
  }
});

test('repo signing path keeps hardened runtime off', () => {
  const executableText = (text) => text
    .split('\n')
    .filter((line) => !line.trimStart().startsWith('#'))
    .join('\n');
  assert.doesNotMatch(executableText(signApp), /--options runtime/);
  assert.doesNotMatch(executableText(signBinary), /--options runtime/);
  assert.doesNotMatch(executableText(buildDeploy), /--options runtime/);
});
