---
title: API reference
description: Every merchant endpoint, request field, response and webhook payload, generated from openapi.yaml.
---

This page is generated from [`openapi.yaml`](https://github.com/loruorg/turmite/blob/main/openapi.yaml) in the repository. The file is an OpenAPI 3.1 document. Load it into your own tools to generate a client or to test requests.

The base URL is your site URL, for example `https://pay.example.com`. Send the project API key in the `x-api-key` header. Payout endpoints also need a JWT from `POST /v1/auth` in the `Authorization: Bearer` header. The [Merchant API guide](/docs/merchant-api/) explains each flow with examples.
