# Embuer

Embuer is a fast, easy-to-use and quick to integrate update daemon especially suited for *embedded* linux systems.

## Generating the keypair

The keypair can be generated using openssl:

```sh
openssl genrsa -out private_key.pem 2048
```

From the private key the public key must be generated in PEM format:

```sh
openssl rsa -in private_key.pem -pubout -outform PEM -RSAPublicKey_out -out public_key_pkcs1.pem
```
