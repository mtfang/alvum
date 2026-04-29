const assert = require('node:assert/strict');
const fs = require('node:fs');
const path = require('node:path');
const test = require('node:test');

const repo = path.join(__dirname, '..', '..');
const signing = fs.readFileSync(path.join(repo, 'scripts', 'signing.sh'), 'utf8');
const signApp = fs.readFileSync(path.join(repo, 'scripts', 'sign-app.sh'), 'utf8');
const signBinary = fs.readFileSync(path.join(repo, 'scripts', 'sign-binary.sh'), 'utf8');
const buildDeploy = fs.readFileSync(path.join(repo, 'scripts', 'build-deploy.sh'), 'utf8');
const main = fs.readFileSync(path.join(repo, 'app', 'main.js'), 'utf8');
const cliPlist = fs.readFileSync(path.join(repo, 'crates', 'alvum-cli', 'Info.plist'), 'utf8');

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

test('repo signing path keeps hardened runtime off', () => {
  const executableText = (text) => text
    .split('\n')
    .filter((line) => !line.trimStart().startsWith('#'))
    .join('\n');
  assert.doesNotMatch(executableText(signApp), /--options runtime/);
  assert.doesNotMatch(executableText(signBinary), /--options runtime/);
  assert.doesNotMatch(executableText(buildDeploy), /--options runtime/);
});
