package basalt.oracle;

import com.google.gson.JsonArray;
import com.google.gson.JsonElement;
import com.google.gson.JsonObject;
import com.google.gson.stream.JsonWriter;
import com.mojang.blaze3d.vertex.QuadInstance;
import com.mojang.blaze3d.vertex.VertexConsumer;
import net.minecraft.client.Minecraft;
import net.minecraft.client.color.block.BlockColors;
import net.minecraft.client.color.block.BlockTintSource;
import net.minecraft.client.model.geom.builders.UVPair;
import net.minecraft.client.renderer.block.BlockStateModelSet;
import net.minecraft.client.renderer.block.FluidModel;
import net.minecraft.client.renderer.block.FluidRenderer;
import net.minecraft.client.renderer.block.FluidStateModelSet;
import net.minecraft.client.renderer.block.ModelBlockRenderer;
import net.minecraft.client.renderer.block.dispatch.BlockStateModel;
import net.minecraft.client.renderer.block.dispatch.BlockStateModelPart;
import net.minecraft.client.renderer.chunk.ChunkSectionLayer;
import net.minecraft.client.renderer.texture.TextureAtlasSprite;
import net.minecraft.client.resources.model.geometry.BakedQuad;
import net.minecraft.core.BlockPos;
import net.minecraft.core.Direction;
import net.minecraft.core.registries.BuiltInRegistries;
import net.minecraft.resources.Identifier;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.CardinalLighting;
import net.minecraft.world.level.block.Block;
import net.minecraft.world.level.block.RenderShape;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.block.state.properties.Property;
import net.minecraft.world.level.material.FluidState;
import org.joml.Vector3fc;

import java.util.*;

/** kind "mesh": runs the client's own block + fluid tesselators on fully specified scenes
 *  and reports every emitted quad. Schema: tools/oracle-mod/SCHEMA.md. */
final class MeshHandler implements Handler {
    static final Direction[] DIRS = Direction.values();

    @Override public void handle(JsonObject req, JsonWriter w) throws Exception {
        Minecraft mc = Minecraft.getInstance();
        BlockStateModelSet models = mc.getModelManager().getBlockStateModelSet();
        FluidStateModelSet fluids = mc.getModelManager().getFluidStateModelSet();
        BlockColors colors = mc.getBlockColors();
        JsonObject opt = req.has("options") ? req.getAsJsonObject("options") : new JsonObject();
        boolean ao = !opt.has("ao") || opt.get("ao").getAsBoolean();
        boolean cull = !opt.has("cull") || opt.get("cull").getAsBoolean();
        ModelBlockRenderer renderer = new ModelBlockRenderer(ao, cull, colors);
        FluidRenderer fluidRenderer = new FluidRenderer(fluids);
        w.name("options").beginObject().name("ao").value(ao).name("cull").value(cull).endObject();
        w.name("scenes").beginArray();
        long quads = 0;
        for (JsonElement se : req.getAsJsonArray("scenes")) {
            JsonObject s = se.getAsJsonObject();
            w.beginObject();
            w.name("id").value(s.get("id").getAsLong());
            List<Quad> out = new ArrayList<>();
            String error = null;
            try { meshScene(s, models, fluids, colors, renderer, fluidRenderer, out); }
            catch (Throwable e) { error = e.toString(); }
            if (error != null) w.name("error").value(error);
            w.name("quads").beginArray();
            for (Quad q : out) q.write(w);
            w.endArray();
            w.endObject();
            quads += out.size();
        }
        w.endArray();
        w.name("total_quads").value(quads);
    }

    static final class Vert { float x, y, z, u, v; int color = -1, light; }

    static final class Quad {
        String src, layer, dir, cull, tex; int tintIndex = -1; Integer tint; boolean shade; int emission;
        Vert[] v = new Vert[4];
        void write(JsonWriter w) throws Exception {
            w.beginObject();
            w.name("src").value(src);
            w.name("layer").value(layer);
            w.name("dir").value(dir);
            if (cull != null) w.name("cull").value(cull);
            w.name("tex").value(tex);
            w.name("ti").value(tintIndex);
            if (tint != null) w.name("tint").value(tint & 0xFFFFFF);
            w.name("shade").value(shade);
            if (emission != 0) w.name("emit").value(emission);
            w.name("v").beginArray();
            for (Vert x : v) {
                w.beginArray();
                w.value(r(x.x)).value(r(x.y)).value(r(x.z)).value(r(x.u)).value(r(x.v));
                w.value(x.color & 0xFFFFFFFFL).value(x.light);
                w.endArray();
            }
            w.endArray();
            w.endObject();
        }
    }
    static double r(float f) { return Math.round(f * 100000.0) / 100000.0; }

    static BlockState parseState(String s) {
        int b = s.indexOf('[');
        String name = b < 0 ? s : s.substring(0, b);
        Identifier id = Identifier.parse(name);
        Block block = BuiltInRegistries.BLOCK.getValue(id);
        if (!BuiltInRegistries.BLOCK.getKey(block).equals(id)) throw new IllegalArgumentException("unknown block " + name);
        BlockState st = block.defaultBlockState();
        if (b >= 0) {
            String body = s.substring(b + 1, s.lastIndexOf(']'));
            for (String kv : body.split(",")) {
                if (kv.isEmpty()) continue;
                String[] p = kv.split("=", 2);
                Property<?> prop = null;
                for (Property<?> q : block.getStateDefinition().getProperties()) if (q.getName().equals(p[0])) prop = q;
                if (prop == null) throw new IllegalArgumentException("no property " + p[0] + " on " + name);
                st = with(st, prop, p[1]);
            }
        }
        return st;
    }
    static <T extends Comparable<T>> BlockState with(BlockState st, Property<T> p, String v) {
        return st.setValue(p, p.getValue(v).orElseThrow(() -> new IllegalArgumentException("bad value " + v + " for " + p.getName())));
    }

    static SceneLevel buildLevel(JsonObject s, BlockPos origin) {
        SceneLevel lvl = new SceneLevel();
        for (JsonElement be : s.getAsJsonArray("blocks")) {
            JsonArray a = be.getAsJsonArray();
            BlockPos p = origin.offset(a.get(0).getAsInt(), a.get(1).getAsInt(), a.get(2).getAsInt());
            lvl.blocks.put(p.asLong(), parseState(a.get(3).getAsString()));
        }
        if (s.has("light")) {
            JsonObject l = s.getAsJsonObject("light");
            if (l.has("default")) { JsonArray d = l.getAsJsonArray("default"); lvl.defaultLight = d.get(0).getAsInt() << 4 | d.get(1).getAsInt(); }
            if (l.has("cells")) for (JsonElement ce : l.getAsJsonArray("cells")) {
                JsonArray a = ce.getAsJsonArray();
                BlockPos p = origin.offset(a.get(0).getAsInt(), a.get(1).getAsInt(), a.get(2).getAsInt());
                lvl.light.put(p.asLong(), a.get(3).getAsInt() << 4 | a.get(4).getAsInt());
            }
        }
        if (s.has("tint")) {
            JsonObject t = s.getAsJsonObject("tint");
            if (t.has("grass")) lvl.grass = t.get("grass").getAsInt();
            if (t.has("foliage")) lvl.foliage = t.get("foliage").getAsInt();
            if (t.has("dry_foliage")) lvl.dryFoliage = t.get("dry_foliage").getAsInt();
            if (t.has("water")) lvl.water = t.get("water").getAsInt();
        }
        if (s.has("cardinal") && s.get("cardinal").getAsString().equals("nether")) lvl.cardinal = CardinalLighting.NETHER;
        return lvl;
    }

    static void meshScene(JsonObject s, BlockStateModelSet models, FluidStateModelSet fluids, BlockColors colors,
                          ModelBlockRenderer renderer, FluidRenderer fluidRenderer, List<Quad> out) {
        JsonArray o = s.has("origin") ? s.getAsJsonArray("origin") : null;
        BlockPos origin = o == null ? new BlockPos(0, 64, 0) : new BlockPos(o.get(0).getAsInt(), o.get(1).getAsInt(), o.get(2).getAsInt());
        SceneLevel lvl = buildLevel(s, origin);
        BlockState st = lvl.getBlockState(origin);

        if (st.getRenderShape() == RenderShape.MODEL) {
            BlockStateModel model = models.get(st);
            long seed = st.getSeed(origin);
            // which cull-face list each quad came from (identity), over several variant picks
            IdentityHashMap<BakedQuad, String> cullOf = new IdentityHashMap<>();
            for (long sd : new long[]{seed, 0, 1, 2, 3, 42}) {
                List<BlockStateModelPart> parts = new ArrayList<>();
                model.collectParts(RandomSource.create(sd), parts);
                for (BlockStateModelPart part : parts) {
                    for (Direction d : DIRS) for (BakedQuad q : part.getQuads(d)) cullOf.put(q, d.getName());
                    for (BakedQuad q : part.getQuads(null)) cullOf.putIfAbsent(q, null);
                }
            }
            renderer.tesselateBlock((x, y, z, q, inst) -> out.add(blockQuad(x, y, z, q, inst, cullOf, colors, st, lvl, origin)),
                    0f, 0f, 0f, lvl, origin, st, model, seed);
        }

        FluidState fs = st.getFluidState();
        if (!fs.isEmpty()) {
            FluidModel fm = fluids.get(fs);
            Map<ChunkSectionLayer, Capture> caps = new LinkedHashMap<>();
            fluidRenderer.tesselate(lvl, origin, layer -> caps.computeIfAbsent(layer, Capture::new), st, fs);
            float ox = origin.getX() & 15, oy = origin.getY() & 15, oz = origin.getZ() & 15;
            List<TextureAtlasSprite> sprites = new ArrayList<>();
            if (fm.stillMaterial() != null) sprites.add(fm.stillMaterial().sprite());
            if (fm.flowingMaterial() != null) sprites.add(fm.flowingMaterial().sprite());
            if (fm.overlayMaterial() != null) sprites.add(fm.overlayMaterial().sprite());
            for (Capture c : caps.values()) {
                for (int i = 0; i + 3 < c.verts.size(); i += 4) {
                    Quad q = new Quad();
                    q.src = "fluid"; q.layer = c.layer.name().toLowerCase(Locale.ROOT); q.shade = true;
                    float cu = 0, cv = 0;
                    for (int k = 0; k < 4; k++) { Vert v = c.verts.get(i + k); cu += v.u / 4; cv += v.v / 4; }
                    TextureAtlasSprite sp = null;
                    for (TextureAtlasSprite cand : sprites) if (cu >= cand.getU0() && cu <= cand.getU1() && cv >= cand.getV0() && cv <= cand.getV1()) sp = cand;
                    q.tex = sp == null ? "?" : sp.contents().name().toString();
                    for (int k = 0; k < 4; k++) {
                        Vert v = c.verts.get(i + k), n = new Vert();
                        n.x = v.x - ox; n.y = v.y - oy; n.z = v.z - oz; n.color = v.color; n.light = v.light;
                        n.u = sp == null ? v.u : (v.u - sp.getU0()) / (sp.getU1() - sp.getU0());
                        n.v = sp == null ? v.v : (v.v - sp.getV0()) / (sp.getV1() - sp.getV0());
                        q.v[k] = n;
                    }
                    Vert n0 = c.normals.get(i);
                    q.dir = Direction.getApproximateNearest(n0.x, n0.y, n0.z).getName();
                    out.add(q);
                }
            }
        }
    }

    static Quad blockQuad(float x, float y, float z, BakedQuad bq, QuadInstance inst, IdentityHashMap<BakedQuad, String> cullOf,
                          BlockColors colors, BlockState st, SceneLevel lvl, BlockPos origin) {
        Quad q = new Quad();
        var mi = bq.materialInfo();
        TextureAtlasSprite sp = mi.sprite();
        q.src = "block";
        q.layer = mi.layer().name().toLowerCase(Locale.ROOT);
        q.dir = bq.direction().getName();
        q.cull = cullOf.containsKey(bq) ? cullOf.get(bq) : "?";
        q.tex = sp.contents().name().toString();
        q.tintIndex = mi.tintIndex();
        q.shade = mi.shade();
        q.emission = mi.lightEmission();
        if (mi.tintIndex() >= 0) {
            BlockTintSource src = colors.getTintSource(st, mi.tintIndex());
            if (src != null) q.tint = src.colorInWorld(st, lvl, origin);
        }
        float du = sp.getU1() - sp.getU0(), dv = sp.getV1() - sp.getV0();
        for (int i = 0; i < 4; i++) {
            Vector3fc p = bq.position(i);
            long uv = bq.packedUV(i);
            Vert v = new Vert();
            v.x = x + p.x(); v.y = y + p.y(); v.z = z + p.z();
            v.u = (UVPair.unpackU(uv) - sp.getU0()) / du;
            v.v = (UVPair.unpackV(uv) - sp.getV0()) / dv;
            v.color = inst.getColor(i);
            v.light = inst.getLightCoords(i);
            q.v[i] = v;
        }
        return q;
    }

    /** Captures what the fluid renderer writes into a vertex consumer. */
    static final class Capture implements VertexConsumer {
        final ChunkSectionLayer layer;
        final List<Vert> verts = new ArrayList<>(), normals = new ArrayList<>();
        Vert cur, curN;
        Capture(ChunkSectionLayer l) { layer = l; }
        @Override public VertexConsumer addVertex(float x, float y, float z) {
            cur = new Vert(); cur.x = x; cur.y = y; cur.z = z; verts.add(cur);
            curN = new Vert(); normals.add(curN);
            return this;
        }
        @Override public VertexConsumer setColor(int r, int g, int b, int a) { cur.color = (a & 255) << 24 | (r & 255) << 16 | (g & 255) << 8 | (b & 255); return this; }
        @Override public VertexConsumer setColor(int argb) { cur.color = argb; return this; }
        @Override public VertexConsumer setUv(float u, float v) { cur.u = u; cur.v = v; return this; }
        @Override public VertexConsumer setUv1(int u, int v) { return this; }
        @Override public VertexConsumer setUv2(int u, int v) { cur.light = (v & 0xFFFF) << 16 | (u & 0xFFFF); return this; }
        @Override public VertexConsumer setNormal(float x, float y, float z) { curN.x = x; curN.y = y; curN.z = z; return this; }
        @Override public VertexConsumer setLineWidth(float w) { return this; }
    }
}
