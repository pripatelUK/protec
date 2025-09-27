import express from 'express';
import bodyParser from 'body-parser';
import cors from 'cors';
import dotenv from 'dotenv';
import * as SimpleWebAuthnServer from '@simplewebauthn/server';

dotenv.config();
const app = express();
app.use(cors({ origin: '*' }));
app.use(bodyParser.urlencoded({ extended: false }));
app.use(bodyParser.json());

const users = new Map();
const challenges = new Map();

const rpId = process.env.RP_ID || 'pripateluk.github.io';
const expectedOrigin = (process.env.RP_ORIGINS || 'https://pripateluk.github.io').split(',');

function getNewChallenge() {
    return Math.random().toString(36).substring(2);
}
function convertChallenge(chal) {
    return Buffer.from(chal, 'utf8').toString('base64').replace(/=/g, '');
}

app.post('/Register/start', (req, res) => {
    const { username } = req.body;
    if (!username) return res.status(400).json({ error: 'username required' });
    const challenge = getNewChallenge();
    challenges.set(username, convertChallenge(challenge));
    const pubKey = {
        challenge,
        rp: { id: rpId, name: 'webauthn-app' },
        user: { id: username, name: username, displayName: username },
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
    return res.json(pubKey);
});

app.post('/register/finish', async (req, res) => {
    const { username, data } = req.body;
    if (!username) return res.status(400).json({ error: 'username required' });
    try {
        const verification = await SimpleWebAuthnServer.verifyRegistrationResponse({
            response: data,
            expectedChallenge: challenges.get(username),
            expectedOrigin,
        });
        const { verified, registrationInfo } = verification;
        if (verified) {
            users.set(username, registrationInfo);
            return res.json(true);
        }
        return res.status(500).json(false);
    } catch (e) {
        return res.status(400).json({ error: String(e) });
    }
});

app.post('/login/start', (req, res) => {
    const { username } = req.body;
    if (!users.has(username)) return res.status(404).json(false);
    const challenge = getNewChallenge();
    challenges.set(username, convertChallenge(challenge));
    const user = users.get(username);
    return res.json({
        challenge,
        rpId,
        allowCredentials: [{ type: 'public-key', id: user.credentialID, transports: ['internal'] }],
        userVerification: 'preferred',
    });
});

app.post('/login/finish', async (req, res) => {
    const { username, data } = req.body;
    if (!users.has(username)) return res.status(404).json(false);
    try {
        const verification = await SimpleWebAuthnServer.verifyAuthenticationResponse({
            expectedChallenge: challenges.get(username),
            response: data,
            authenticator: users.get(username),
            expectedRPID: rpId,
            expectedOrigin,
            requireUserVerification: false,
        });
        return res.json({ res: verification.verified });
    } catch (e) {
        return res.status(400).json({ error: String(e) });
    }
});

const port = Number(process.env.PORT || 3000);
app.listen(port, () => {
    console.log('Passkeys bun server listening on', port, 'rpId', rpId, 'origins', expectedOrigin);
});
