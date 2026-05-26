import json

from sdk.python import SimeisSDK

def test_app():
    sdk = SimeisSDK("UtilisateurDeTest", "localhost", "8080")
    status = sdk.get_player_status()
    money = status["money"]
    sta = status["stations"][0]
    ships = sdk.shop_list_ship(sta)
    ship_to_buy = ships[0]
    sdk.buy_ship(sta, ship_to_buy["id"])
    status_aftership = sdk.get_player_status()
    money_aftership = status_aftership["money"]
    sdk.buy_module_on_ship(sta, ship_to_buy["id"], "Miner")
    status_aftermod = sdk.get_player_status()
    money_aftermod = status_aftermod["money"]

    assert money > money_aftership
    assert money_aftership > money_aftermod



