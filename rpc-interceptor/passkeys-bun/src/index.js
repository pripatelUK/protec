import express from 'express';
import bodyParser from 'body-parser';
import cors from 'cors';
import dotenv from 'dotenv';
import * as SimpleWebAuthnServer from '@simplewebauthn/server';
import crypto from 'crypto';

dotenv.config();
const app = express();
app.use(cors({ origin: '*' }));
app.use(bodyParser.urlencoded({ extended: false }));
app.use(bodyParser.json());

const users = new Map();
const challenges = new Map();

const rpId = process.env.RP_ID || 'pripateluk.github.io';
const expectedOrigin = (process.env.RP_ORIGINS || 'https://pripateluk.github.io').split(',');

function b64urlNoPad(buf) {
    return Buffer.from(buf)
        .toString('base64')
        .replace(/\+/g, '-')
        .replace(/\//g, '_')
        .replace(/=+$/g, '');
}
function getNewChallenge() {
    return b64urlNoPad(crypto.randomBytes(32));
}
function convertChallenge(chalB64Url) {
    // Store exactly what we send to the client
    return chalB64Url;
}

app.post('/api/passkeys/register/start', (req, res) => {
    const { email, display_name } = req.body;
    if (!email) return res.status(400).json({ error: 'email required' });
    const challenge = getNewChallenge();
    challenges.set(email, convertChallenge(challenge));
    const pubKey = {
        challenge,
        rp: { id: rpId, name: 'Protec' },
        user: { id: Buffer.from(email, 'utf8').toString('base64'), name: email, displayName: display_name || email },
        pubKeyCredParams: [
            { type: 'public-key', alg: -7 },
            { type: 'public-key', alg: -257 },
        ],
        authenticatorSelection: {
            authenticatorAttachment: 'platform',
            userVerification: 'required',
            residentKey: 'preferred',
            requireResidentKey: false,
        },
    };
    return res.json({ publicKey: pubKey });
});

app.post('/api/passkeys/register/finish', async (req, res) => {
    const { email, id, rawId, type, response } = req.body;
    if (!email) return res.status(400).json({ error: 'email required' });
    try {
        const verification = await SimpleWebAuthnServer.verifyRegistrationResponse({
            response: { id, rawId, type, response },
            expectedChallenge: challenges.get(email),
            expectedOrigin,
        });
        const { verified, registrationInfo } = verification;
        if (verified) {
            users.set(email, registrationInfo);
            return res.json({ ok: true });
        }
        return res.status(500).json({ ok: false });
    } catch (e) {
        return res.status(400).json({ ok: false, error: String(e) });
    }
});

app.post('/api/passkeys/assert/start', (req, res) => {
    const { email } = req.body || {};
    const challenge = getNewChallenge();
    const reply = { challenge, rpId, userVerification: 'required' };
    if (email && users.has(email)) {
        const user = users.get(email);
        reply.allowCredentials = [{ type: 'public-key', id: user.credentialID, transports: ['internal'] }];
    }
    challenges.set(email || '__discoverable__', convertChallenge(challenge));
    return res.json({ publicKey: reply });
});

app.post('/api/passkeys/assert/finish', async (req, res) => {
    const { email, id, rawId, type, response } = req.body;
    let authenticator;
    if (email && users.has(email)) {
        authenticator = users.get(email);
    } else {
        for (const [, info] of users.entries()) {
            if (Buffer.compare(Buffer.from(info.credentialID), Buffer.from(rawId, 'base64')) === 0) {
                authenticator = info;
                break;
            }
        }
    }
    if (!authenticator) return res.status(404).json({ ok: false, error: 'authenticator_not_found' });
    try {
        const verification = await SimpleWebAuthnServer.verifyAuthenticationResponse({
            expectedChallenge: challenges.get(email || '__discoverable__'),
            response: { id, rawId, type, response },
            authenticator,
            expectedRPID: rpId,
            expectedOrigin,
            requireUserVerification: false,
        });
        if (!verification.verified) return res.status(400).json({ ok: false });
        return res.json({ ok: true, assertion_token: 'demo' });
    } catch (e) {
        return res.status(400).json({ ok: false, error: String(e) });
    }
});

app.post('/api/sessions/resume', (req, res) => {
    return res.json({ ok: true, session_id: 'demo', rpc_endpoint: 'http://localhost:3000/rpc/demo' });
});

const port = Number(process.env.PORT || 3000);
app.listen(port, () => {
    console.log('Passkeys bun server listening on', port, 'rpId', rpId, 'origins', expectedOrigin);
});
