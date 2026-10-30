# Connections

[Contents](README.md)

## TCP

The server takes TCP connections at the hosts or addresses of
SERVER.HOST, by commas, 127.0.0.1 by default, on SERVER.PORT, 7422; port
0 lets the system choose one, which the server says as it starts.
SERVER.HOST empty takes no TCP connection. Over TCP, a password is sent
only once TLS secures the connection.

## Unix sockets

SERVER.SOCKET names a Unix socket the server takes connections at too,
on Linux and macOS. A connection over it is secured by the system, so a
password may be sent over it.

```
$ turboreld --dir db --set SERVER.SOCKET=/run/turborel/socket
$ turborel --socket /run/turborel/socket -U ann -W
```

## TLS

SERVER.TLS ON offers TLS on the TCP port; REQUIRED takes TCP connections
over TLS alone. SERVER.TLS_CERTIFICATE names the PEM file of the
server's certificate chain, SERVER.TLS_KEY that of its private key. A
client trusts the certificates of the PEM file it is given, `--tls`, and
checks the server's certificate against the host it names, or
`--server-name`.

```
$ turboreld --dir db --set SERVER.TLS=REQUIRED \
      --set SERVER.TLS_CERTIFICATE=/etc/turborel/cert.pem --set SERVER.TLS_KEY=/etc/turborel/key.pem
$ turborel -h localhost -U ann -W --tls /etc/turborel/ca.pem
```

A client that does not use TLS is refused, and so is one that does not
trust the certificate:

```
turborel: SQLCODE -919 (SQLSTATE 08004): this server takes TCP connections over TLS only
turborel: SQLCODE -919 (SQLSTATE 08006): the TLS handshake failed: invalid peer certificate: UnknownIssuer
```

The server's certificate is one a certificate authority issued, as the
client trusts: a self-signed certificate used as its own authority is
refused.

## Limits on connections

| Setting | Limit |
|---|---|
| SERVER.MAX_CONNECTIONS | connections at once, 256 |
| SERVER.WORKERS | statements run at once, one per processor |
| SERVER.MAX_STATEMENTS | prepared statements of a connection, 1,024 |
| SERVER.MAX_MESSAGE_BYTES | the longest message a client sends, 16 MiB |
| SERVER.BATCH_BYTES | about the bytes of rows in a batch sent, 64 KiB |
| SERVER.LOGIN_TIMEOUT | the milliseconds a client has to begin TLS and authenticate, 60,000 |
| SERVER.IDLE_TIMEOUT | the milliseconds the server waits on a client before closing it, its transaction rolled back; 0, no limit |

A connection past SERVER.MAX_CONNECTIONS is refused, told why once its
TLS, if any, is begun:

```
turborel: SQLCODE -919 (SQLSTATE 08004): the server has as many connections as it takes
```

At most 16 such refusals are made at once; a connection past them too
is closed at once, told nothing, so clients that flood the server hold
no more of it than that.
