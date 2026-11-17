"""Exchange rates (a slow remote call in production)."""
RATES = {"USD": 1.0, "EUR": 0.9, "GBP": 0.8}
CALLS = []


def rate(currency):
    CALLS.append(currency)
    return RATES[currency]
