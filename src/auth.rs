use axum::Json;
use jsonwebtoken::{encode, EncodingKey, Header};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct Claims {
    sub: String,
    exp: usize,
}

pub async fn login(Json(payload): Json<LoginRequest>) -> Json<String> {
    if payload.username == "admin" && payload.password == "admin" {
        let claims = Claims {
            sub: "admin".into(),
            exp: 2000000000,
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret("secret".as_ref()),
        )
        .unwrap();

        Json(token)
    } else {
        Json("Unauthorized".into())
    }
}
