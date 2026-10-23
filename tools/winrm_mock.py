#!/usr/bin/env python3
"""Mock WinRM server for testing Aurora's WinRM client without a Windows machine.

Speaks just enough WS-Management: NTLMv2 (verifies the NTProofStr and the MIC, so the
client's crypto is checked independently), Basic over HTTPS, and the shell calls
Create / Command / Receive / Signal / Delete. PowerShell is *not* run: the script (decoded
from -EncodedCommand) is matched against a few markers and canned output is returned.

  winrm_mock.py --port 5985 [--user admin --password secret --domain CORP]
                [--tls CERT KEY] [--basic] [--no-timestamp] [--unencrypted-only]
"""
import argparse, base64, hashlib, hmac, os, re, struct, sys, threading, time, uuid
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import ssl

# ----------------------------------------------------------------------------- crypto

def md4(data: bytes) -> bytes:
    def rol(x, n): return ((x << n) | (x >> (32 - n))) & 0xFFFFFFFF
    msg = bytearray(data)
    ml = len(data) * 8
    msg.append(0x80)
    while len(msg) % 64 != 56:
        msg.append(0)
    msg += struct.pack('<Q', ml)
    a, b, c, d = 0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476
    for off in range(0, len(msg), 64):
        X = struct.unpack('<16I', msg[off:off + 64])
        aa, bb, cc, dd = a, b, c, d
        F = lambda x, y, z: (x & y) | (~x & z)
        G = lambda x, y, z: (x & y) | (x & z) | (y & z)
        H = lambda x, y, z: x ^ y ^ z
        for i in range(16):
            k = i; s = [3, 7, 11, 19][i % 4]
            t = rol((a + F(b, c, d) + X[k]) & 0xFFFFFFFF, s)
            a, d, c, b = d, c, b, t
        for i in range(16):
            k = [0, 4, 8, 12, 1, 5, 9, 13, 2, 6, 10, 14, 3, 7, 11, 15][i]; s = [3, 5, 9, 13][i % 4]
            t = rol((a + G(b, c, d) + X[k] + 0x5a827999) & 0xFFFFFFFF, s)
            a, d, c, b = d, c, b, t
        for i in range(16):
            k = [0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15][i]; s = [3, 9, 11, 15][i % 4]
            t = rol((a + H(b, c, d) + X[k] + 0x6ed9eba1) & 0xFFFFFFFF, s)
            a, d, c, b = d, c, b, t
        a, b, c, d = (a + aa) & 0xFFFFFFFF, (b + bb) & 0xFFFFFFFF, (c + cc) & 0xFFFFFFFF, (d + dd) & 0xFFFFFFFF
    return struct.pack('<4I', a, b, c, d)

def hmac_md5(k, m): return hmac.new(k, m, hashlib.md5).digest()
def u16(s): return s.encode('utf-16-le')

def buf(msg, off):
    ln, _, o = struct.unpack_from('<HHI', msg, off)
    return msg[o:o + ln]

def av_pairs(b):
    out, i = [], 0
    while i + 4 <= len(b):
        t, l = struct.unpack_from('<HH', b, i)
        if t == 0: break
        out.append((t, b[i + 4:i + 4 + l])); i += 4 + l
    return out

# ----------------------------------------------------------------------------- canned output

FACTS = ("@@os\r\ncaption=Microsoft Windows Server 2022 Standard\r\nversion=10.0.20348\r\nbuild=20348\r\narch=64-bit\r\n"
         "uptime=93784\r\nhost=MOCK-WIN01\r\ndomain=corp.example.com\r\npartofdomain=True\r\nmodel=Mock Inc. Virtual Machine\r\n"
         "ramkb=16776692\r\nfreekb=8388346\r\ncpu=Intel(R) Xeon(R) Gold 6248 CPU @ 2.50GHz\r\nsockets=2\r\nlogical=8\r\nuser=CORP\\alice\r\n"
         "@@disks\r\nC:\tSystem\tNTFS\t107374182400\t32212254720\r\nE:\tData\tReFS\t1099511627776\t549755813888\r\n"
         "@@addr\r\nEthernet\t10.0.0.5/24\r\nEthernet\t2001:db8::5/64\r\n")
SERVICES = ("Spooler\tPrint Spooler\tRunning\tAutomatic\t0\r\nWinDefend\tMicrosoft Defender Antivirus Service\tStopped\tAutomatic\t1\r\n"
            "MyApp\tMy Application Service\tStopped\tAutomatic\t1053\r\nBITS\tBackground Intelligent Transfer Service\tStopped\tManual\t0\r\n"
            "W32Time\tWindows Time\tRunning\tManual\t0\r\n")
PORTS = ("tcp\t0.0.0.0\t135\tsvchost\r\ntcp\t0.0.0.0\t445\tSystem\r\ntcp\t0.0.0.0\t3389\tsvchost\r\ntcp\t0.0.0.0\t5985\tSystem\r\nudp\t0.0.0.0\t123\tsvchost\r\n")
FIREWALL = ("@@profiles\r\nDomain\tTrue\tBlock\tAllow\r\nPrivate\tTrue\tBlock\tAllow\r\nPublic\tFalse\tBlock\tAllow\r\n@@rules\r\n"
            "Remote Desktop (TCP-In)\tInbound\tAllow\tTCP\t3389\t\tAny\r\nBlock telnet\tInbound\tBlock\tTCP\t23\t\tAny\r\n"
            "Windows Remote Management (HTTP-In)\tInbound\tAllow\tTCP\t5985\tSystem\tDomain, Private\r\n")

def run_script(script: str, state):
    """(stdout, stderr_clixml, exit, delay_seconds)"""
    if 'Win32_OperatingSystem' in script: return FACTS, '', 0, 0
    if 'Get-Service' in script: return SERVICES, '', 0, 0
    if 'Get-NetTCPConnection' in script: return PORTS, '', 0, 0
    if 'Get-NetFirewallProfile' in script: return FIREWALL, '', 0, 0
    if 'Microsoft.Update.Session' in script: return 'updates=4\r\nsecurity=1\r\n', '', 0, 0
    m = re.search(r'Start-Sleep\s+(\d+)', script)
    delay = int(m.group(1)) if m else 0
    if 'throw' in script:
        return '', '#< CLIXML\r\n<Objs Version="1.1.0.1" xmlns="http://schemas.microsoft.com/powershell/2004/04"><S S="Error">mock failure_x000D__x000A_</S></Objs>', 1, delay
    m = re.search(r'exit\s+(\d+)', script)
    code = int(m.group(1)) if m else 0
    body = script.split(';', 3)[-1].strip()
    return 'mock-output ' + body + '\r\n' + 'second line ünï\r\n', '', code, delay

# ----------------------------------------------------------------------------- server

NS = ('xmlns:s="http://www.w3.org/2003/05/soap-envelope" xmlns:a="http://schemas.xmlsoap.org/ws/2004/08/addressing" '
      'xmlns:w="http://schemas.dmtf.org/wbem/wsman/1/wsman.xsd" xmlns:rsp="http://schemas.microsoft.com/wbem/wsman/1/windows/shell"')

def env(body): return f'<s:Envelope {NS}><s:Header/><s:Body>{body}</s:Body></s:Envelope>'

def fault(code, text):
    return env(f'<s:Fault><s:Code><s:Value>s:Receiver</s:Value></s:Code><s:Reason><s:Text xml:lang="en-US">{text}</s:Text></s:Reason>'
               f'<s:Detail><f:WSManFault xmlns:f="http://schemas.microsoft.com/wbem/wsman/1/wsmanfault" Code="{code}"><f:Message>{text}</f:Message></f:WSManFault></s:Detail></s:Fault>')

ARGS = None
SHELLS = {}      # shell id -> {}
COMMANDS = {}    # command id -> dict
LOG = []

class H(BaseHTTPRequestHandler):
    protocol_version = 'HTTP/1.1'

    def log_message(self, fmt, *a):
        sys.stderr.write('[mock] ' + fmt % a + '\n')

    # --- helpers
    def reply(self, code, body=b'', headers=()):
        if isinstance(body, str): body = body.encode()
        self.send_response(code)
        for k, v in headers: self.send_header(k, v)
        self.send_header('Content-Type', 'application/soap+xml;charset=UTF-8')
        self.send_header('Content-Length', str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def challenge(self):
        self.server_challenge = os.urandom(8)
        info = b''
        def av(t, v): return struct.pack('<HH', t, len(v)) + v
        info += av(2, u16('CORP')) + av(1, u16('MOCK-WIN01')) + av(4, u16('corp.example.com'))
        if not ARGS.no_timestamp:
            self.ts = struct.pack('<Q', (int(time.time()) + 11644473600) * 10**7)
            info += av(7, self.ts)
        info += b'\0\0\0\0'
        self.target_info = info
        tn = u16('CORP')
        flags = 0xe2898205 | 0x00800000
        hdr = b'NTLMSSP\0' + struct.pack('<I', 2) + struct.pack('<HHI', len(tn), len(tn), 56) + struct.pack('<I', flags) \
            + self.server_challenge + b'\0' * 8 + struct.pack('<HHI', len(info), len(info), 56 + len(tn)) + bytes([6, 1, 0xb1, 0x1d, 0, 0, 0, 15])
        self.chal_msg = hdr + tn + info
        return self.chal_msg

    def check_auth3(self, msg):
        if msg[:8] != b'NTLMSSP\0' or struct.unpack_from('<I', msg, 8)[0] != 3: return 'not type 3'
        lm, nt, dom, usr = buf(msg, 12), buf(msg, 20), buf(msg, 28).decode('utf-16-le'), buf(msg, 36).decode('utf-16-le')
        if usr.lower() != ARGS.user.lower(): return f'bad user {usr!r}'
        proof, temp = nt[:16], nt[16:]
        ntowf = hmac_md5(md4(u16(ARGS.password)), u16(usr.upper() + dom))
        want = hmac_md5(ntowf, self.server_challenge + temp)
        if proof != want: return 'NTProofStr mismatch (wrong password or broken NTLMv2)'
        # temp: 1,1,0,0,0,0,0,0 + time(8) + cc(8) + 4 zero + target info + 4 zero
        pairs = av_pairs(temp[28:])
        if not ARGS.no_timestamp:
            if dict(pairs).get(7) != self.ts: return 'client did not echo the timestamp'
            if lm != b'\0' * 24: return 'LM response should be zero with timestamp'
            flags = struct.unpack('<I', dict(pairs).get(6, b'\0\0\0\0'))[0]
            if not flags & 2: return 'MsvAvFlags MIC bit missing'
            mic = msg[72:88]
            z = bytearray(msg); z[72:88] = b'\0' * 16
            sbk = hmac_md5(ntowf, want)
            if hmac_md5(sbk, self.neg_msg + self.chal_msg + bytes(z)) != mic: return 'MIC mismatch'
        return None

    def do_POST(self):
        n = int(self.headers.get('Content-Length', 0))
        body = self.rfile.read(n) if n else b''
        if self.path != '/wsman': return self.reply(404)
        auth = self.headers.get('Authorization', '')
        if ARGS.basic:
            want = 'Basic ' + base64.b64encode(f'{ARGS.user}:{ARGS.password}'.encode()).decode()
            if auth != want:
                return self.reply(401, headers=[('WWW-Authenticate', 'Basic realm="WSMAN"')])
        else:
            if getattr(self, 'authed', False) is not True:
                if not auth.startswith(('Negotiate ', 'NTLM ')):
                    return self.reply(401, headers=[('WWW-Authenticate', 'Negotiate'), ('WWW-Authenticate', 'Basic realm="WSMAN"')])
                msg = base64.b64decode(auth.split(' ', 1)[1])
                t = struct.unpack_from('<I', msg, 8)[0] if msg[:8] == b'NTLMSSP\0' else 0
                if t == 1:
                    self.neg_msg = msg
                    return self.reply(401, headers=[('WWW-Authenticate', 'Negotiate ' + base64.b64encode(self.challenge()).decode())])
                err = self.check_auth3(msg) if t == 3 and hasattr(self, 'chal_msg') else 'unexpected message'
                if err:
                    sys.stderr.write(f'[mock] AUTH FAILED: {err}\n')
                    return self.reply(401, headers=[('WWW-Authenticate', 'Negotiate')])
                self.authed = True
                sys.stderr.write('[mock] NTLM authentication OK\n')
        if ARGS.unencrypted_only is False and False: pass
        text = body.decode('utf-8', 'replace')
        action = re.search(r'<a:Action[^>]*>([^<]+)</a:Action>', text)
        action = action.group(1) if action else ''
        sys.stderr.write(f'[mock] {action.rsplit("/", 1)[-1]}\n')
        LOG.append(action)
        if action.endswith('/transfer/Create'):
            sid = str(uuid.uuid4()).upper(); SHELLS[sid] = {}
            return self.reply(200, env(f'<rsp:Shell><rsp:ShellId>{sid}</rsp:ShellId></rsp:Shell>'))
        sel = re.search(r'Name="ShellId">([^<]+)<', text)
        sid = sel.group(1) if sel else ''
        if sid not in SHELLS: return self.reply(500, fault(2150858843, 'The shell was not found'))
        if action.endswith('/shell/Command'):
            args = re.findall(r'<rsp:Arguments>([^<]*)</rsp:Arguments>', text)
            if '-EncodedCommand' not in args: return self.reply(500, fault(1, 'expected -EncodedCommand'))
            enc = args[args.index('-EncodedCommand') + 1]
            script = base64.b64decode(enc).decode('utf-16-le')
            out, err, code, delay = run_script(script, SHELLS[sid])
            cid = str(uuid.uuid4()).upper()
            COMMANDS[cid] = dict(out=out, err=err, code=code, ready=time.time() + delay, sent=False)
            sys.stderr.write(f'[mock] script ({len(script)} chars): {script[:90]!r}\n')
            return self.reply(200, env(f'<rsp:CommandResponse><rsp:CommandId>{cid}</rsp:CommandId></rsp:CommandResponse>'))
        if action.endswith('/shell/Receive'):
            cid = re.search(r'CommandId="([^"]+)"', text).group(1)
            c = COMMANDS.get(cid)
            if not c: return self.reply(500, fault(2150858843, 'unknown command'))
            if time.time() < c['ready']:
                time.sleep(min(2, c['ready'] - time.time()))
                if time.time() < c['ready']:
                    return self.reply(500, fault(2150858793, 'The WS-Management service cannot complete the operation within the time specified in OperationTimeout.'))
            b64 = lambda s: base64.b64encode(s.encode()).decode()
            parts = ''
            if c['out']: parts += f'<rsp:Stream Name="stdout" CommandId="{cid}">{b64(c["out"])}</rsp:Stream>'
            if c['err']: parts += f'<rsp:Stream Name="stderr" CommandId="{cid}">{b64(c["err"])}</rsp:Stream>'
            parts += (f'<rsp:CommandState CommandId="{cid}" State="http://schemas.microsoft.com/wbem/wsman/1/windows/shell/CommandState/Done">'
                      f'<rsp:ExitCode>{c["code"]}</rsp:ExitCode></rsp:CommandState>')
            return self.reply(200, env(f'<rsp:ReceiveResponse>{parts}</rsp:ReceiveResponse>'))
        if action.endswith('/shell/Signal'):
            cid = re.search(r'CommandId="([^"]+)"', text).group(1)
            COMMANDS.pop(cid, None)
            return self.reply(200, env('<rsp:SignalResponse/>'))
        if action.endswith('/transfer/Delete'):
            SHELLS.pop(sid, None)
            return self.reply(200, env(''))
        return self.reply(500, fault(1, 'unsupported action ' + action))

def main():
    global ARGS
    ap = argparse.ArgumentParser()
    ap.add_argument('--port', type=int, default=5985)
    ap.add_argument('--user', default='admin')
    ap.add_argument('--password', default='S3cret!pw')
    ap.add_argument('--domain', default='')
    ap.add_argument('--tls', nargs=2, metavar=('CERT', 'KEY'))
    ap.add_argument('--basic', action='store_true')
    ap.add_argument('--no-timestamp', action='store_true')
    ap.add_argument('--unencrypted-only', action='store_true')
    ARGS = ap.parse_args()
    srv = ThreadingHTTPServer(('127.0.0.1', ARGS.port), H)
    if ARGS.tls:
        ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        ctx.load_cert_chain(*ARGS.tls)
        srv.socket = ctx.wrap_socket(srv.socket, server_side=True)
    print(f'mock WinRM on 127.0.0.1:{ARGS.port} user={ARGS.user} tls={bool(ARGS.tls)} basic={ARGS.basic}', flush=True)
    srv.serve_forever()

if __name__ == '__main__':
    main()
