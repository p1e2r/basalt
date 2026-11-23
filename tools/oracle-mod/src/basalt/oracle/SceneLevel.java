package basalt.oracle;

import net.minecraft.client.renderer.BiomeColors;
import net.minecraft.client.renderer.block.BlockAndTintGetter;
import net.minecraft.core.BlockPos;
import net.minecraft.core.SectionPos;
import net.minecraft.world.level.BlockGetter;
import net.minecraft.world.level.CardinalLighting;
import net.minecraft.world.level.ColorResolver;
import net.minecraft.world.level.LightLayer;
import net.minecraft.world.level.block.Blocks;
import net.minecraft.world.level.block.entity.BlockEntity;
import net.minecraft.world.level.block.state.BlockState;
import net.minecraft.world.level.chunk.DataLayer;
import net.minecraft.world.level.chunk.LightChunk;
import net.minecraft.world.level.chunk.LightChunkGetter;
import net.minecraft.world.level.lighting.LayerLightEventListener;
import net.minecraft.world.level.lighting.LevelLightEngine;
import net.minecraft.world.level.material.FluidState;
import net.minecraft.world.level.ChunkPos;

import java.util.HashMap;
import java.util.Map;

/** A tiny fully-specified world for one scene: explicit block states, explicit per-cell light,
 *  explicit biome colours. Everything not listed is air with the scene's default light. */
final class SceneLevel implements BlockAndTintGetter {
    final Map<Long, BlockState> blocks = new HashMap<>();
    /** packed sky << 4 | block */
    final Map<Long, Integer> light = new HashMap<>();
    int defaultLight = 15 << 4;
    int grass = 0x91BD59, foliage = 0x77AB2F, dryFoliage = 0xA0784D, water = 0x3F76E4;
    CardinalLighting cardinal = CardinalLighting.DEFAULT;

    private final LevelLightEngine engine;

    SceneLevel() {
        SceneLevel self = this;
        LightChunkGetter getter = new LightChunkGetter() {
            @Override public LightChunk getChunkForLighting(int x, int z) { return null; }
            @Override public BlockGetter getLevel() { return self; }
        };
        engine = new LevelLightEngine(getter, false, false) {
            @Override public LayerLightEventListener getLayerListener(LightLayer layer) { return new Listener(layer); }
            @Override public int getRawBrightness(BlockPos pos, int skyDarken) { return Math.max(self.sky(pos) - skyDarken, self.blockLight(pos)); }
        };
    }

    int packed(BlockPos p) { Integer v = light.get(p.asLong()); return v == null ? defaultLight : v; }
    int sky(BlockPos p) { return packed(p) >> 4 & 15; }
    int blockLight(BlockPos p) { return packed(p) & 15; }

    final class Listener implements LayerLightEventListener {
        final LightLayer layer;
        Listener(LightLayer l) { layer = l; }
        @Override public DataLayer getDataLayerData(SectionPos pos) { return null; }
        @Override public int getLightValue(BlockPos pos) { return layer == LightLayer.SKY ? sky(pos) : blockLight(pos); }
        @Override public void checkBlock(BlockPos pos) {}
        @Override public boolean hasLightWork() { return false; }
        @Override public int runLightUpdates() { return 0; }
        @Override public void updateSectionStatus(SectionPos pos, boolean empty) {}
        @Override public void setLightEnabled(ChunkPos pos, boolean on) {}
        @Override public void propagateLightSources(ChunkPos pos) {}
    }

    @Override public LevelLightEngine getLightEngine() { return engine; }
    @Override public int getBrightness(LightLayer layer, BlockPos pos) { return layer == LightLayer.SKY ? sky(pos) : blockLight(pos); }
    @Override public int getRawBrightness(BlockPos pos, int skyDarken) { return Math.max(sky(pos) - skyDarken, blockLight(pos)); }
    @Override public boolean canSeeSky(BlockPos pos) { return sky(pos) >= 15; }

    @Override public CardinalLighting cardinalLighting() { return cardinal; }

    @Override public int getBlockTint(BlockPos pos, ColorResolver r) {
        if (r == BiomeColors.GRASS_COLOR_RESOLVER) return grass;
        if (r == BiomeColors.FOLIAGE_COLOR_RESOLVER) return foliage;
        if (r == BiomeColors.DRY_FOLIAGE_COLOR_RESOLVER) return dryFoliage;
        if (r == BiomeColors.WATER_COLOR_RESOLVER) return water;
        throw new IllegalStateException("unknown colour resolver " + r);
    }

    @Override public BlockEntity getBlockEntity(BlockPos pos) { return null; }
    @Override public BlockState getBlockState(BlockPos pos) { BlockState s = blocks.get(pos.asLong()); return s == null ? Blocks.AIR.defaultBlockState() : s; }
    @Override public FluidState getFluidState(BlockPos pos) { return getBlockState(pos).getFluidState(); }
    @Override public int getHeight() { return 384; }
    @Override public int getMinY() { return -64; }
}
