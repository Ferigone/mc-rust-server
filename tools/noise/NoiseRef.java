import java.nio.file.Files;
import java.nio.file.Path;
import java.util.List;
import java.util.stream.IntStream;
import net.minecraft.SharedConstants;
import net.minecraft.server.Bootstrap;
import net.minecraft.util.RandomSource;
import net.minecraft.world.level.levelgen.DensityFunction;
import net.minecraft.world.level.levelgen.LegacyRandomSource;
import net.minecraft.world.level.levelgen.PositionalRandomFactory;
import net.minecraft.world.level.levelgen.XoroshiroRandomSource;
import net.minecraft.world.level.levelgen.synth.BlendedNoise;
import net.minecraft.world.level.levelgen.synth.ImprovedNoise;
import net.minecraft.world.level.levelgen.synth.NormalNoise;
import net.minecraft.world.level.levelgen.synth.PerlinNoise;
import net.minecraft.world.level.levelgen.synth.SimplexNoise;

/**
 * Runs the REAL Mojang random sources and noise functions through a fixed call sequence and prints exact values
 * ("<name>\t<signed decimal of the raw bits>"). crates/mc-worldgen/tests/noise_parity.rs replays the same sequence.
 */
public class NoiseRef {
    static final StringBuilder OUT = new StringBuilder();

    static void e(String k, long v) {
        OUT.append(k).append('\t').append(v).append('\n');
    }

    static void d(String k, double v) {
        e(k, Double.doubleToRawLongBits(v));
    }

    static void f(String k, float v) {
        e(k, Float.floatToRawIntBits(v));
    }

    static final long[] SEEDS = {0L, 1L, -1L, 12345L, 0x0123456789ABCDEFL, Long.MIN_VALUE, Long.MAX_VALUE, 2718281828459045L};
    static final long[] NOISE_SEEDS = {0L, 12345L, -1L};
    static final double[][] POINTS = {
        {0, 0, 0}, {0.5, 0.5, 0.5}, {1.234, -5.678, 9.1011}, {100.1, 64.7, -200.3},
        {-1000.001, 12.5, 3333.3}, {3.3554432E7 + 0.5, 0, 0}, {-0.0001, -0.0001, -0.0001}, {12345.6789, -64.0, -9876.54321},
    };
    static final int[][] BLOCKS = {{0, 0, 0}, {100, 64, -100}, {-17, -5, 333}, {1234, 200, -4321}, {-30000000, -64, 30000000}};

    static RandomSource make(String kind, long seed) {
        return kind.equals("legacy") ? new LegacyRandomSource(seed) : new XoroshiroRandomSource(seed);
    }

    static void random(String kind, long seed) {
        RandomSource r = make(kind, seed);
        String p = "rnd." + kind + "." + seed + ".";
        e(p + "int", r.nextInt());
        e(p + "int7", r.nextInt(7));
        e(p + "int16", r.nextInt(16));
        e(p + "int256", r.nextInt(256));
        e(p + "int1000003", r.nextInt(1000003));
        e(p + "long", r.nextLong());
        e(p + "bool", r.nextBoolean() ? 1 : 0);
        f(p + "float", r.nextFloat());
        d(p + "double", r.nextDouble());
        d(p + "gauss1", r.nextGaussian());
        d(p + "gauss2", r.nextGaussian());
        d(p + "gauss3", r.nextGaussian());
        e(p + "between", r.nextIntBetweenInclusive(-5, 5));
        d(p + "triangle", r.triangle(1.0, 2.0));
        RandomSource fk = r.fork();
        e(p + "fork.long", fk.nextLong());
        PositionalRandomFactory pf = r.forkPositional();
        e(p + "pos.at1", pf.at(1, -2, 3).nextLong());
        e(p + "pos.at2", pf.at(-30000000, 64, 30000000).nextLong());
        e(p + "pos.hash1", pf.fromHashOf("minecraft:test").nextLong());
        e(p + "pos.hash2", pf.fromHashOf("octave_-7").nextLong());
        e(p + "pos.hash3", pf.fromHashOf("\u00fcn\u00ef:\uD83D\uDC8E").nextLong());
        e(p + "pos.seed", pf.fromSeed(5).nextLong());
        r.setSeed(seed + 1);
        e(p + "reseed", r.nextLong());
        r.consumeCount(10);
        e(p + "consume", r.nextLong());
    }

    static void improved(String kind, long seed) {
        ImprovedNoise n = new ImprovedNoise(make(kind, seed));
        String p = "improved." + kind + "." + seed + ".";
        d(p + "xo", n.xo);
        d(p + "yo", n.yo);
        d(p + "zo", n.zo);
        for (int i = 0; i < POINTS.length; i++) {
            double[] q = POINTS[i];
            d(p + i + ".noise", n.noise(q[0], q[1], q[2]));
            d(p + i + ".scaled", n.noise(q[0], q[1], q[2], 0.5, 1.0));
            double[] der = new double[3];
            d(p + i + ".deriv", n.noiseWithDerivative(q[0], q[1], q[2], der));
            d(p + i + ".dx", der[0]);
            d(p + i + ".dy", der[1]);
            d(p + i + ".dz", der[2]);
        }
    }

    static void perlin(String name, PerlinNoise n, String p) {
        d(p + name + ".max", n.maxBrokenValue(0.0));
        for (int i = 0; i < POINTS.length; i++) {
            double[] q = POINTS[i];
            d(p + name + "." + i, n.getValue(q[0], q[1], q[2]));
        }
    }

    static void noises(long seed) {
        String p = "noise." + seed + ".";
        perlin("perlin", PerlinNoise.create(new XoroshiroRandomSource(seed), -7, 1.0, 1.0, 0.0, 1.5, 0.5), p);
        perlin("perlin_octaves", PerlinNoise.create(new XoroshiroRandomSource(seed), List.of(-4, -3, -1, 0)), p);
        perlin("perlin_legacy_blended", PerlinNoise.createLegacyForBlendedNoise(new XoroshiroRandomSource(seed), IntStream.rangeClosed(-15, 0)), p);
        NormalNoise nn = NormalNoise.create(new XoroshiroRandomSource(seed), -6, 1.0, 1.0, 0.5, 0.0, 0.25);
        d(p + "normal.max", nn.maxValue());
        NormalNoise nl = NormalNoise.createLegacyNetherBiome(new LegacyRandomSource(seed), new NormalNoise.NoiseParameters(-7, 1.0, 1.0));
        d(p + "normal_legacy.max", nl.maxValue());
        SimplexNoise sx = new SimplexNoise(new LegacyRandomSource(seed));
        for (int i = 0; i < POINTS.length; i++) {
            double[] q = POINTS[i];
            d(p + "normal." + i, nn.getValue(q[0], q[1], q[2]));
            d(p + "normal_legacy." + i, nl.getValue(q[0], q[1], q[2]));
            d(p + "simplex2." + i, sx.getValue(q[0], q[2]));
            d(p + "simplex3." + i, sx.getValue(q[0], q[1], q[2]));
        }
        BlendedNoise bn = new BlendedNoise(new XoroshiroRandomSource(seed), 0.25, 0.125, 80.0, 160.0, 8.0);
        d(p + "blended.min", bn.minValue());
        d(p + "blended.max", bn.maxValue());
        for (int i = 0; i < BLOCKS.length; i++) {
            int[] b = BLOCKS[i];
            d(p + "blended." + i, bn.compute(new DensityFunction.SinglePointContext(b[0], b[1], b[2])));
        }
    }

    public static void main(String[] args) throws Exception {
        SharedConstants.tryDetectVersion();
        Bootstrap.bootStrap();
        for (long s : SEEDS) {
            random("legacy", s);
            random("xoroshiro", s);
        }
        for (long s : NOISE_SEEDS) {
            improved("xoroshiro", s);
            improved("legacy", s);
            noises(s);
        }
        Files.writeString(Path.of(args[0]), OUT.toString());
        System.out.println("RESULT: wrote " + OUT.chars().filter(c -> c == '\n').count() + " values to " + args[0]);
    }
}
