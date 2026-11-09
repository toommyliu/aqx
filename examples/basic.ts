import * as Api from "aqx";

export default function* (): Api.Script {
    Api.settings.room = 1;
    yield* Api.map.join("farm", { cell: "End" });
    yield* Api.quests.accept(11);
    while (Api.inventory.count("Treeant Branch") !== 1) {
        yield* Api.combat.kill("Treeant");
    }
}