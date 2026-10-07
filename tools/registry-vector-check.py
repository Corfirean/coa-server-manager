"""Independent check of the documented Registry test vector with the RFC 8032 reference algorithm (pure Python, not the code under test)."""
import base64
import hashlib

p = 2**255 - 19
q = 2**252 + 27742317777372353535851937790883648493
d = -121665 * pow(121666, p - 2, p) % p
I = pow(2, (p - 1) // 4, p)


def sha512(s):
    return hashlib.sha512(s).digest()


def inv(x):
    return pow(x, p - 2, p)


def xrecover(y):
    xx = (y * y - 1) * inv(d * y * y + 1)
    x = pow(xx, (p + 3) // 8, p)
    if (x * x - xx) % p != 0:
        x = (x * I) % p
    if x % 2 != 0:
        x = p - x
    return x


By = 4 * inv(5) % p
Bx = xrecover(By)
B = (Bx % p, By % p, 1, (Bx * By) % p)


def add(P, Q):
    A = (P[1] - P[0]) * (Q[1] - Q[0]) % p
    Bv = (P[1] + P[0]) * (Q[1] + Q[0]) % p
    C = 2 * P[3] * Q[3] * d % p
    D = 2 * P[2] * Q[2] % p
    E, F, G, H = Bv - A, D - C, D + C, Bv + A
    return (E * F % p, G * H % p, F * G % p, E * H % p)


def mul(s, P):
    Q = (0, 1, 1, 0)
    while s > 0:
        if s & 1:
            Q = add(Q, P)
        P = add(P, P)
        s >>= 1
    return Q


def enc(P):
    zi = inv(P[2])
    x, y = P[0] * zi % p, P[1] * zi % p
    return int.to_bytes(y | ((x & 1) << 255), 32, "little")


def clamp(h):
    a = int.from_bytes(h[:32], "little")
    a &= (1 << 254) - 8
    a |= 1 << 254
    return a


def public(seed):
    return enc(mul(clamp(sha512(seed)), B))


def sign(seed, msg):
    h = sha512(seed)
    a = clamp(h)
    A = enc(mul(a, B))
    r = int.from_bytes(sha512(h[32:] + msg), "little") % q
    R = enc(mul(r, B))
    k = int.from_bytes(sha512(R + A + msg), "little") % q
    S = (r + k * a) % q
    return R + int.to_bytes(S, 32, "little")


b64 = lambda b: base64.urlsafe_b64encode(b).decode().rstrip("=")
seed = bytes([7]) * 32
realm = "018f2d9e-5c3a-7b21-8c4d-0e5f6a7b8c9d"
body = b'{"protocol_version":1}'
msg = "\n".join(["coa-registry-sig-v1", "1", "POST", f"/registry/v1/realms/{realm}/heartbeat", realm, "1790000000", hashlib.sha256(body).hexdigest()]).encode()
print("public key ", b64(public(seed)))
print("signature  ", b64(sign(seed, msg)))
