"""Loopback-only HTTPS peer for CardDAV tests; all credentials are synthetic.

Two modes share one process, driven by JSON lines on stdin. The first line of
stdout always reports {"port", "certificate"}.

Scripted mode (default): every request pops the next {"status", "body",
"location"} from the queue set by {"responses": [...]}; an empty queue answers
500. Used by the Rust runtime tests.

Serve mode ({"serve": true}): a small directory server answers real requests
from the map below, so the GUI and the API contract can connect, discover and
refresh against it. Every request must carry Basic authorization. REPORT
answers only a valid addressbook-query: like Apple CalendarServer, a query
without the mandatory RFC 6352 filter is refused with 400. A one-shot fault
can be armed for a path with {"faults": {"/dav/directory/": 503}}; the next
request to that path answers with that status. {"responses": [...} still
takes precedence while its queue is non-empty. {"requests": ...} in every
reply line reports the requests seen since it was last read.
"""
import http.server
import json
import ssl
import subprocess
import sys
import tempfile
import threading
from pathlib import Path


with tempfile.TemporaryDirectory(prefix="omamail-carddav-tls-") as directory:
    cert, key = Path(directory) / "cert.pem", Path(directory) / "key.pem"
    request, extensions = Path(directory) / "request.pem", Path(directory) / "extensions.cnf"
    # The peer presents this certificate as an end entity, so it must say
    # CA:FALSE — and say it once. `req -x509` adds its own basicConstraints
    # (CA:TRUE, which webpki refuses on a leaf), and LibreSSL on macOS keeps
    # it beside an -addext copy, a duplicate the OpenSSL that Python loads
    # the chain with refuses outright. Signing a request with an explicit
    # extension file yields exactly these extensions on either toolkit.
    # The IP SAN lets a process that cannot pin DNS use the loopback address;
    # dav.example.org stays for the resolver-pinned Rust tests.
    extensions.write_text(
        "basicConstraints=critical,CA:FALSE\n"
        "subjectAltName=DNS:dav.example.org,DNS:localhost,DNS:carddav-peer.invalid,IP:127.0.0.1\n")
    for command in ([
        "openssl", "req", "-new", "-newkey", "rsa:2048", "-nodes",
        "-keyout", str(key), "-out", str(request), "-subj", "/CN=dav.example.org",
    ], [
        "openssl", "x509", "-req", "-in", str(request), "-signkey", str(key),
        "-out", str(cert), "-days", "1", "-extfile", str(extensions),
    ]):
        subprocess.run(command, check=True, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10)
    state = {"responses": [], "requests": [], "connections": 0, "serve": False, "faults": {}}

    PRINCIPAL = (
        '<d:multistatus xmlns:d="DAV:"><d:response><d:href>/dav/</d:href>'
        '<d:propstat><d:prop><d:current-user-principal>'
        '<d:href>/dav/pal/synthetic%40example.org/</d:href>'
        '</d:current-user-principal></d:prop>'
        '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>')
    HOME = (
        '<d:multistatus xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:carddav">'
        '<d:response><d:href>/dav/pal/synthetic%40example.org/</d:href>'
        '<d:propstat><d:prop><c:addressbook-home-set>'
        '<d:href>/dav/</d:href>'
        '</c:addressbook-home-set></d:prop>'
        '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response></d:multistatus>')
    BOOK_HOME = (
        '<d:response><d:href>/dav/</d:href>'
        '<d:propstat><d:prop><d:displayname>Synthetic home</d:displayname>'
        '<d:resourcetype><d:collection/></d:resourcetype></d:prop>'
        '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>')
    BOOK_PERSONAL = (
        '<d:response><d:href>/dav/personal/</d:href>'
        '<d:propstat><d:prop><d:displayname>Personal contacts</d:displayname>'
        '<d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop>'
        '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>')
    BOOK_DIRECTORY = (
        '<d:response><d:href>/dav/directory/</d:href>'
        '<d:propstat><d:prop><d:displayname>Справочник компании</d:displayname>'
        '<d:resourcetype><d:collection/><c:addressbook/></d:resourcetype></d:prop>'
        '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>')
    def multistatus(responses):
        return ('<d:multistatus xmlns:d="DAV:" '
                'xmlns:c="urn:ietf:params:xml:ns:carddav">%s</d:multistatus>' % responses)
    BOOKS = multistatus(BOOK_HOME + BOOK_PERSONAL + BOOK_DIRECTORY)

    def vcard(name, *emails):
        addresses = "".join(
            "EMAIL;TYPE=%s:%s\n" % (kind, address) for kind, address in emails)
        return ("BEGIN:VCARD\nVERSION:4.0\nFN:%s\n%sEND:VCARD\n" % (name, addresses))

    def report(*cards):
        responses = "".join(
            '<d:response><d:href>/dav/card%d.vcf</d:href>'
            '<d:propstat><d:prop><d:getetag>"%d"</d:getetag>'
            '<c:address-data>%s</c:address-data></d:prop>'
            '<d:status>HTTP/1.1 200 OK</d:status></d:propstat></d:response>'
            % (index, index, card.replace("\n", "&#10;"))
            for index, card in enumerate(cards))
        return ('<d:multistatus xmlns:d="DAV:" '
                'xmlns:c="urn:ietf:params:xml:ns:carddav">%s</d:multistatus>' % responses)

    PERSONAL_BOOK = report(
        vcard("Иван Петров", ("work", "ivan.petrov@example.org"), ("home", "ivan@example.org")),
        vcard("Мария Сидорова", ("work", "maria.sidorova@example.org")),
        vcard("Ольга Ким", ("work", "olga.kim@example.org")))
    DIRECTORY_BOOK = report(
        vcard("Елена Волкова", ("work", "elena.volkova@example.org")),
        vcard("Дмитрий Соколов", ("work", "dmitry.sokolov@example.org")),
        vcard("Анна Мельникова", ("work", "anna.melnikova@example.org")),
        vcard("Pavel Svetlov", ("work", "pavel.svetlov@example.org")))

    def serve_answer(method, path, depth, body):
        if not path.startswith("/dav/"):
            return 404, ""
        if method == "REPORT":
            # RFC 6352 10.3: a query without the mandatory filter is invalid;
            # strict servers refuse it, and so does this peer.
            if "filter" not in body:
                return 400, ""
            if path == "/dav/personal/":
                return 207, PERSONAL_BOOK
            if path == "/dav/directory/":
                return 207, DIRECTORY_BOOK
            return 404, ""
        if method == "PROPFIND":
            if path == "/dav/pal/synthetic%40example.org/":
                return 207, HOME
            if depth == "1" and path == "/dav/":
                return 207, BOOKS
            if path == "/dav/":
                return 207, PRINCIPAL
            if path == "/dav/personal/":
                return 207, multistatus(BOOK_PERSONAL)
            if path == "/dav/directory/":
                return 207, multistatus(BOOK_DIRECTORY)
            return 404, ""
        return 405, ""

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_GET(self):
            length = int(self.headers.get("Content-Length", "0"))
            state["requests"].append({
                "host": self.headers.get("Host"), "path": self.path,
                "method": self.command, "authorization": self.headers.get("Authorization"),
                "depth": self.headers.get("Depth"),
                "body": self.rfile.read(length).decode(),
            })
            response = {"status": 500}
            if state["responses"]:
                response = state["responses"].pop(0)
            elif state["serve"]:
                fault = state["faults"].pop(self.path, None)
                if fault is not None:
                    response = {"status": fault}
                elif not (self.headers.get("Authorization") or "").startswith("Basic "):
                    response = {"status": 401}
                else:
                    status, body = serve_answer(
                        self.command, self.path, self.headers.get("Depth"),
                        state["requests"][-1]["body"])
                    response = {"status": status, "body": body}
            body = response.get("body", "").encode()
            self.send_response(response.get("status", 200))
            if "location" in response:
                self.send_header("Location", response["location"])
            self.send_header("Content-Type", "application/xml; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        do_PROPFIND = do_GET
        do_REPORT = do_GET

    context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    context.load_cert_chain(cert, key)

    class Server(http.server.ThreadingHTTPServer):
        def get_request(self):
            connection, address = super().get_request()
            state["connections"] += 1
            connection.settimeout(3)
            try:
                return context.wrap_socket(connection, server_side=True), address
            except OSError:
                connection.close()
                raise

    server = Server(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    print(json.dumps({"port": server.server_port, "certificate": str(cert)}), flush=True)
    for line in sys.stdin:
        command = json.loads(line)
        if command.get("stop"):
            break
        if "serve" in command:
            state["serve"] = bool(command["serve"])
        if "faults" in command:
            state["faults"] = {str(path): int(status)
                               for path, status in command["faults"].items()}
        if "responses" in command:
            state["responses"] = command["responses"]
            state["requests"] = []
        print(json.dumps({"requests": state["requests"],
                          "connections": state["connections"]}), flush=True)
