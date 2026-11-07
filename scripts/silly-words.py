#!/usr/bin/env python3
"""Display-only text filter: python3 scripts/silly-words.py < input.txt.

Pipe a command into this script to filter its output in Neoism's terminal.
This does not install a Neoism UI filter or change the source text.
"""

import argparse
import json
import re
import sys
from collections import Counter
from pathlib import Path


# Explicit variants avoid stemming innocent words such as "class" or "cumulative".
# This dictionary necessarily contains offensive terms so it can match them.
GROUPS = {
    "fudge": """
        fuck fucks fucked fucker fuckers fucking fuckin fuckhead fuckheads
        fuckface fuckfaces fuckwit fuckwits fuckup fuckups fuckoff fuckboy
        fuckboys motherfuck motherfucker motherfuckers motherfucking
        motherfucked mofo mofos fuk fukk fuking fukin fck wtf stfu gtfo
        fuckery fucktard fucktards fuckstick fucksticks fucknut fucknuts
        fuckbag fuckbags fuckfest fuckfests clusterfuck clusterfucks
        mindfuck mindfucked mindfucking butt fuckwad fuckwads fuckwitted
        motherfuckin motherfucka motherfuckas mothafucka mothafuckas
        mothafucker mothafuckers motherfucking's fubar af frigging friggin
        fricking frickin feck fecking effing effed
    """,
    "beans": """
        shit shits shitty shittier shittiest shitting shitted shite shites
        bullshit bullshits bullshitting horseshit batshit chickenshit dipshit
        dipshits shithead shitheads shitface shitfaced shitbag shitbags
        shitshow shitshows shitstorm shitstorms shitload shitloads shithole
        shitholes shitstain shitstains shitfuck shitfucker shat bs
        shitheads shitbird shitbirds shitheel shitheels shitass shitasses
        shitcan shitcanned shitcanning shitlist shitlists shitless shitballs
        shitter shitters shittiest shitfucks shitfuckers bullshitters
        bullshitter bullshitted bullshitty dogshit apeshit ratshit
        shit-eating shit-faced shit-for-brains piece-of-shit
    """,
    "wafflehead": """
        ass asses asshole assholes arse arses arsehole arseholes dumbass
        dumbasses jackass jackasses badass badasses smartass smartasses
        asswipe asswipes asshat asshats assclown assclowns assface
        bastard bastards bitch bitches bitching bitched bitchy biatch
        bitchass sonofabitch sonofabitches douche douchebag douchebags
        douchecanoe douchecanoes twat twats twatwaffle tosser tossers
        wanker wankers bellend bellends knobhead knobheads prick pricks
        arsewipe arsewipes arsehat arsehats arseface arsefaces lardass
        fatass fatasses candyass candyasses wiseass wiseasses halfass
        halfassed half-assed jackasses buttface butthead buttheads butthole
        buttholes butt-wipe butt-wipes asslicker asslickers asslicking
        asskisser asskissers asskissing assfucker assfuckers assfucking
        bitchface bitchfaces bitchfest bitchslap bitchslapped bitchslapping
        bitchiness bitchin biznatch beotch beyotch bastardy bastardized
        bastardisation bastardization douchey douchebags knobend knobends
        nobhead nobheads numbnuts numbskull twatwaffles twatface twatfaces
    """,
    "apple juice": """
        piss pisses pissed pissing pisser pissers pissy pissoff pisshead
        pissheads pisspot pisspots pissant pissants
        pisspoor pissflaps pissface pissfaces pissbag pissbags piss-take
        piss-takes piss-off pissed-off piss-poor
    """,
    "frosting": """
        cum cums cumming cummed cumshot cumshots cumslut cumsluts
        cumdump cumdumpster jizz jizzes jizzing jizzed jism spunk spunking
        ejaculate ejaculates ejaculated ejaculating ejaculation ejaculations
        precum pre-cum cumrag cumrags cumstain cumstains cumdumps
        cumdumpsters cumbucket cumbuckets cumface cumfaces cumguzzler
        cumguzzlers cummin jizzum jiz jizzy jizzrag jizzstain spunked spunks
    """,
    "noodle": """
        dick dicks dickhead dickheads dickface dickwad dickwads dickbag
        cock cocks cockhead cockheads cockface cocksucker cocksuckers
        cocksucking cockwomble penis penises boner boners schlong schlongs
        dong dongs pecker peckers prickhead ballsack ballsacks nutsack nutsacks
        bollock bollocks bollocking testicle testicles scrotum scrotums
        dickbags dickweasel dickweasels dickish dickless dickhole dickholes
        dickwads dickheads dickpic dickpics cockbag cockbags cockbite
        cockbites cockwaffle cockwaffles cockwombles cockblock cockblocked
        cockblocking cockring cockrings foreskin foreskins willy willies
        willie johnson johnsons nuts balls bollocking bollockings
    """,
    "cupcake": """
        cunt cunts cunty pussy pussies pussys pussybitch vagina vaginas
        vulva vulvas clit clits clitoris boob boobs boobie boobies tit tits
        titty titties tittie titfuck titfucking titwank
        cunting cuntface cuntfaces cuntbag cuntbags cunthole cuntholes
        cuntish pussypounder pussypounders boobies boobage bazongas
        breasts breast nipple nipples areola areolas areolae snatch snatches
        cooch coochie coochies coochy minge minges fanny fannies muff muffs
    """,
    "goose nuggets": """
        sex sexual sexually sexy sext sexts sexting porn porno pornos
        pornography pornographic pornstar pornstars hentai blowjob blowjobs
        handjob handjobs rimjob rimjobs footjob footjobs anal analingus
        anilingus cunnilingus fellatio deepthroat deepthroating gangbang
        gangbangs gangbanged gangbanging bukkake bukake bukakke orgy orgies
        masturbate masturbates masturbated masturbating masturbation
        wank wanks wanked wanking jerkoff jerkoffs jackingoff jackoff
        orgasm orgasms orgasmic dildo dildos dildoes buttplug buttplugs
        fleshlight fleshlights strap-on strap-ons strap-on's
        intercourse semen sperm sperms smegma queef queefs queefing
        fetish fetishes bdsm bondage bestiality zoophilia necrophilia
        whore whores whoring whorish slut sluts slutty skank skanks skanky
        hooker hookers hookers' hoe hoes thot thots rape rapes raped
        raping rapist rapists incest incestuous pedophile pedophiles
        paedophile paedophiles pedophilia paedophilia
        blow-job blow-jobs hand-job hand-jobs rim-job rim-jobs dryhump
        dryhumping hump humping humped humps shag shags shagged shagging
        shagging's shagfest shagfests shagathon bonk bonks bonking bonked
        boink boinks boinking boinked screwing screw screwjob screwjobs
        fisting fistfuck fistfucking fingerbang fingerbanging fingering
        fellate fellates fellated fellating sodomy sodomize sodomized
        sodomizing sodomise sodomised sodomising sodomite sodomites
        buggering buggery pegging rimming rimmer rimmers rimming's
        masturbator masturbators masturbatory masturbations onanism
        jerk-off jerk-offs jack-off jack-offs jerked jerking jacking
        orgasmically orgasmed orgasming erotic erotica eroticism
        pornographic pornographer pornographers cybersex phone-sex
        sexcam sexcams sexted sexts sexting nudes nude nudity naked
        dominatrix dominatrices dominatrixes sadomasochism sadomasochistic
        bestial zoophile zoophiles necrophile necrophiles necrophiliac
        necrophiliacs pedo pedos paedo paedos molest molests molested
        molesting molester molesters molestation prostitution prostitute
        prostitutes prostituting whorehouse whorehouses whoremonger
        whoremongers slutbag slutbags slutface slutfaces sluts' skankass
        teabagging tea-bagging bumboy bumboys rentboy rentboys
    """,
    "silly goose": """
        nigger niggers nigga niggas niggah niggahs niggar niggars niglet
        niglets coon coons darkie darkies chink chinks gook gooks spic spics
        spick spicks wetback wetbacks beaner beaners kike kikes heeb heebs
        paki pakis raghead ragheads towelhead towelheads redskin redskins
        injun injuns ziphead zipheads honky honkies cracker crackers
        faggot faggots fag fags faggy faggotry dyke dykes tranny trannies
        shemale shemales retard retards retarded retardation retarding
        mongoloid mongoloids spaz spazz spastic spastics lesbo lesbos
        nignog nignogs wigger wiggers porchmonkey porchmonkeys jigaboo
        jigaboos jungle-bunny jungle-bunnies sandnigger sandniggers
        chinky chinkies chingchong ching-chong slopehead slopeheads
        golliwog golliwogs golliwogg golliwoggs wog wogs dago dagos
        guinea guineas kraut krauts jap japs yid yids zhid zhids
        polack polacks pikey pikeys gyppo gyppos gypo gypos kaffir kaffirs
        kaffre kaffres coolie coolies abo abos boong boongs halfbreed
        halfbreeds half-breed half-breeds redneck rednecks whitey whiteys
        faggit faggits fagot fagots faggotry faglord faglords fagbag fagbags
        carpetmuncher carpetmunchers muffdiver muffdivers he-she she-male
        she-males ladyboy ladyboys retardate retardates fucktard fucktards
        libtard libtards tardo tard tards spazzy spazzes spazzing
    """,
    "heck": """
        damn damns damned damning dammit damnit goddamn goddamned
        goddammit goddamnit hell hells bloody bugger buggers buggered
        buggering crap craps crappy crapping crapped crapper crappers
        damningly damnation goddam goddamnits goddamning goshdarn
        goshdarnit darn darned darnit darnation dang danged dangit
        helluva hellhole hellholes hellish hellishly hellbound hellfire
        god-damn god-damned god-dammit god-damnit bloodyhell bleedinghell
        crappier crappiest craphead crapheads crapola craptastic craptacular
        bollix bollixed bollox cobblers sod sods sodding sodded sodoff
        blimey bleeding blimmin blooming ruddy smeg smeghead smegheads
    """,
}


def default_replacements():
    replacements = {
        word: replacement
        for replacement, words in GROUPS.items()
        for word in words.split()
    }
    # Conservative, explicit disguises; do not match arbitrary punctuation or
    # spaced letters, which can consume innocent text or terminal control codes.
    for replacement, words in {
        "fudge": ["f*ck", "f**k", "f***", "f***ing", "f@ck", "f u c k", "f.u.c.k", "f-ck", "fu*k", "f*cks", "f*cking", "f*cked", "f*cker", "f**king", "f**ked", "f**ker", "phuck", "phucking", "fcuk"],
        "beans": ["sh1t", "sh!t", "sh*t", "s**t", "bullsh*t", "sh1tty", "sh*tty", "sh!tty", "sh!thead", "sh*thead", "bullsh1t", "b.s.", "s.h.i.t"],
        "wafflehead": ["b1tch", "b!tch", "b*tch", "a$$", "a$$hole", "a**hole", "assh0le", "b1tches", "b!tches", "b*tches", "b1tchy", "b*tchy"],
        "cupcake": ["c*nt", "c**t"],
        "silly goose": ["n1gger", "n1gga", "n!gger", "n!gga", "f@ggot", "n1ggers", "n1ggas", "n!ggers", "n!ggas", "n*gger", "n*gga", "n***er", "f@ggots", "f*ggot", "f*ggots"],
        "heck": ["d*mn", "d**n", "d@mn", "h3ll", "h*ll", "d@mmit", "d@mnit"],
    }.items():
        replacements.update(dict.fromkeys(words, replacement))
    return replacements


def load_replacements(path=None):
    replacements = default_replacements()
    if path is not None:
        custom = json.loads(Path(path).read_text(encoding="utf-8"))
        if not isinstance(custom, dict):
            raise ValueError("custom replacements must be a JSON object")
        for word, replacement in custom.items():
            if not word or not isinstance(replacement, (str, type(None))):
                raise ValueError("each word needs a string replacement or null")
            if any(char in word or char in (replacement or "") for char in "\r\n\x1b"):
                raise ValueError("words and replacements cannot contain newlines or ESC")
            if replacement is None:
                replacements.pop(word.casefold(), None)
            else:
                replacements[word.casefold()] = replacement
    return replacements


class WordFilter:
    def __init__(self, replacements):
        self.replacements = replacements
        alternatives = "|".join(
            re.escape(word) for word in sorted(replacements, key=len, reverse=True)
        )
        self.pattern = re.compile(r"(?<!\w)(?ai:" + alternatives + r")(?!\w)") if alternatives else None

    def filter(self, text):
        def replace(match):
            word = match.group()
            replacement = self.replacements[word.casefold()]
            if word.isupper():
                return replacement.upper()
            if word.istitle():
                return replacement.capitalize()
            return replacement

        return self.pattern.sub(replace, text) if self.pattern else text


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--replacements", metavar="JSON", help="custom word-to-replacement object; null disables a word")
    parser.add_argument("--text", help="filter one string instead of reading stdin")
    parser.add_argument("--scan", action="store_true", help="report matched words and counts as JSON instead of replacing text")
    args = parser.parse_args()
    try:
        word_filter = WordFilter(load_replacements(args.replacements))
    except (OSError, ValueError) as error:
        parser.error(str(error))
    try:
        if args.scan:
            counts = Counter()
            lines = [args.text] if args.text is not None else sys.stdin
            if word_filter.pattern is not None:
                for line in lines:
                    counts.update(match.group().casefold() for match in word_filter.pattern.finditer(line))
            print(json.dumps(dict(sorted(counts.items())), indent=2, ensure_ascii=True))
        elif args.text is not None:
            print(word_filter.filter(args.text))
        else:
            for line in sys.stdin:
                sys.stdout.write(word_filter.filter(line))
                sys.stdout.flush()
    except BrokenPipeError:
        # Downstream consumers may stop reading before the producer finishes.
        sys.stdout = open("/dev/null" if sys.platform != "win32" else "NUL", "w")


if __name__ == "__main__":
    main()